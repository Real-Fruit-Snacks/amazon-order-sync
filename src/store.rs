//! The notes folder: walking it, writing notes atomically, the sidecar files
//! (.review-context.json, .price-history.json), product images, the parse
//! cache, and the sync write path (createOrUpdateOrder).

use crate::config::Config;
use crate::gemini::{self, Draft, ReviewItem};
use crate::js::{self, to_str, to_str_or_empty};
use crate::jsdate;
use crate::notes::{self, prop, ItemExtra};
use crate::{elog, log};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const ASSET_TYPES: [(&str, &str); 6] = [
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
];
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
const PRICE_HISTORY_CAP: usize = 90;
const STYLE_EXEMPLAR_POOL: usize = 10;
const STYLE_EXEMPLAR_TTL_MS: i64 = 10 * 60 * 1000;

pub fn asset_type(ext: &str) -> Option<&'static str> {
    ASSET_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

pub fn tmp_suffix() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::fill(&mut b);
    let hex: String = b.iter().map(|x| format!("{:02x}", x)).collect();
    format!(".tmp-{}-{}", std::process::id(), hex)
}

fn tmp_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(tmp_suffix());
    PathBuf::from(s)
}

fn transient(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(windows)]
        Some(5) | Some(32) | Some(33) => true,
        #[cfg(unix)]
        Some(libc::EPERM) | Some(libc::EBUSY) | Some(libc::EACCES) => true,
        _ => false,
    }
}

/// A rename over a file an indexer, antivirus or editor holds open fails for
/// a moment on Windows: retry, then remove the temp file.
pub fn rename_with_retry(from: &Path, to: &Path, tries: u32) -> std::io::Result<()> {
    let mut i = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if i + 1 >= tries || !transient(&e) {
                    let _ = std::fs::remove_file(from);
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(50 * (i as u64 + 1)));
                i += 1;
            }
        }
    }
}

/// Write `content` next to `path` and rename it into place.
pub fn write_atomic(path: &Path, content: &[u8], retry: bool) -> std::io::Result<()> {
    let tmp = tmp_path(path);
    std::fs::write(&tmp, content)?;
    if retry {
        rename_with_retry(&tmp, path, 8)
    } else {
        std::fs::rename(&tmp, path)
    }
}

pub fn io_msg(e: &std::io::Error) -> String {
    e.to_string()
}

/// fs.readdir order: libuv sorts with strcmp on Unix; Windows keeps the
/// directory's own order.
pub fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut v: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    if cfg!(unix) {
        v.sort_by(|a, b| a.file_name().as_encoded_bytes().cmp(b.file_name().as_encoded_bytes()));
    }
    Ok(v)
}

pub fn mtime_ms(md: &std::fs::Metadata) -> f64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as f64 * 1000.0 + d.subsec_nanos() as f64 / 1e6)
        .unwrap_or(0.0)
}

pub fn file_name_str(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Read a file as UTF-8 the way Node does (invalid bytes become U+FFFD).
pub fn read_text(p: &Path) -> std::io::Result<String> {
    let b = std::fs::read(p)?;
    Ok(match String::from_utf8(b) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

struct CacheEntry {
    mtime_ms: f64,
    size: u64,
    sort_key: f64,
    parsed: Value,
}

#[derive(Default)]
struct ScanCache {
    files: IndexMap<PathBuf, CacheEntry>,
    walked: usize,
    reparsed: usize,
}

/// A posted review used as a voice sample: (title, text).
pub type Exemplar = (String, String);

pub struct Store {
    pub notes_path: PathBuf,
    pub gemini_api_key: String,
    pub sync_reminder_days: i64,
    pub review_check_interval_days: i64,
    pub gemini_daily_limit: i64,
    pub review_cutoff_days: i64,
    pub notifications_enabled: bool,
    pub gemini_model: String,
    pub gemini_temperature: f64,
    pub style_exemplars: bool,
    pub gemini_endpoint: gemini::Endpoint,
    exemplar_cache: Mutex<Option<(i64, Vec<Exemplar>)>>,
    scan: Mutex<ScanCache>,
}

pub fn notify(msg: &str) {
    log!("{}", msg);
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("notify-send")
            .arg("Amazon Order Sync")
            .arg(msg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut c| std::thread::spawn(move || c.wait()));
    }
}

impl Store {
    pub fn new(cfg: &Config) -> Store {
        Store {
            notes_path: cfg.notes_path.clone(),
            gemini_api_key: cfg.gemini_api_key.clone(),
            sync_reminder_days: cfg.sync_reminder_days,
            review_check_interval_days: cfg.review_check_interval_days,
            gemini_daily_limit: cfg.gemini_daily_limit,
            review_cutoff_days: cfg.review_cutoff_days,
            notifications_enabled: cfg.notifications_enabled,
            gemini_model: cfg.gemini_model.clone(),
            gemini_temperature: cfg.gemini_temperature,
            style_exemplars: cfg.style_exemplars,
            gemini_endpoint: gemini::Endpoint::from_env(),
            exemplar_cache: Mutex::new(None),
            scan: Mutex::new(ScanCache::default()),
        }
    }

    /// notesPath joined with a vault-relative "a/b/c" path.
    pub fn vault_path(&self, rel: &str) -> PathBuf {
        let mut p = self.notes_path.clone();
        for part in rel.split('/') {
            if !part.is_empty() {
                p.push(part);
            }
        }
        p
    }

    pub fn ensure_folder_exists(&self, rel: &str) {
        if let Err(e) = std::fs::create_dir_all(self.vault_path(rel)) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                elog!("Failed to create folder: {}", e);
            }
        }
    }

    pub fn walk_orders(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = read_dir_sorted(dir) else { return };
            for e in entries {
                let Ok(ft) = e.file_type() else { continue };
                let full = dir.join(e.file_name());
                if ft.is_dir() {
                    walk(&full, out);
                } else if ft.is_file() {
                    out.push(full);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.notes_path.join("Orders"), &mut out);
        out
    }

    pub fn find_order_file(&self, order_id: &str) -> Option<PathBuf> {
        let target = format!("Order-{}.md", order_id);
        self.walk_orders().into_iter().find(|f| file_name_str(f) == target)
    }

    pub fn write_markdown_atomic(&self, path: &Path, content: &str) -> std::io::Result<()> {
        write_atomic(path, content.as_bytes(), true)
    }

    // ------------------------------------------------------------------
    // Product images
    // ------------------------------------------------------------------

    /// Returns the note's image reference: None = undefined, Some(Null) =
    /// skipped by a guard, else the cached assets path or the original URL.
    pub fn download_image(&self, url: Option<&Value>, asin: Option<&Value>) -> Option<Value> {
        let truthy = match url {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            _ => true,
        };
        let u = to_str(url);
        if !truthy || !js::test(js::re(r"^https?:\/\/", "i"), &u) {
            return url.cloned();
        }
        match self.download_image_inner(&u, asin) {
            Ok(v) => v,
            Err(e) => {
                elog!("Failed to download image Error: {}", e);
                url.cloned()
            }
        }
    }

    fn download_image_inner(&self, url: &str, asin: Option<&Value>) -> Result<Option<Value>, String> {
        self.ensure_folder_exists("Orders/assets");
        let assets_dir = self.vault_path("Orders/assets");
        let base = notes::sanitize_id(asin);
        if let Ok(entries) = read_dir_sorted(&assets_dir) {
            for e in entries {
                if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(dot) = name.rfind('.') else { continue };
                if dot == 0 {
                    continue;
                }
                let ext = name[dot + 1..].to_lowercase();
                if name[..dot] == base && asset_type(&ext).is_some() {
                    return Ok(Some(Value::String(format!("assets/{}", name))));
                }
            }
        }
        let agent = agent(Duration::from_secs(10));
        let resp = match agent.get(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {}", code)),
            Err(e) => return Err(format!("fetch failed ({})", e)),
        };
        let ct = resp.header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim().to_lowercase();
        if !ct.starts_with("image/") {
            return Ok(Some(Value::Null));
        }
        let Some((ext, _)) = ASSET_TYPES.iter().find(|(_, t)| *t == ct) else { return Ok(Some(Value::Null)) };
        let declared = js::string_to_number(resp.header("content-length").unwrap_or("0"));
        if declared > MAX_IMAGE_BYTES as f64 {
            return Ok(Some(Value::Null));
        }
        let mut buf = Vec::new();
        resp.into_reader().take(MAX_IMAGE_BYTES + 1).read_to_end(&mut buf).map_err(|e| format!("fetch failed ({})", e))?;
        if buf.len() as u64 > MAX_IMAGE_BYTES {
            return Ok(Some(Value::Null));
        }
        let file_name = format!("{}.{}", base, ext);
        std::fs::write(assets_dir.join(&file_name), &buf).map_err(|e| io_msg(&e))?;
        Ok(Some(Value::String(format!("assets/{}", file_name))))
    }

    // ------------------------------------------------------------------
    // Voice-of-customer sidecar
    // ------------------------------------------------------------------

    fn review_context_file(&self) -> PathBuf {
        self.notes_path.join(".review-context.json")
    }

    fn clean_reviews(v: &Value) -> Option<Vec<Value>> {
        let a = v.as_array()?;
        let out: Vec<Value> = a
            .iter()
            .filter_map(|r| r.as_str())
            .map(|r| js::utf16_prefix(js::trim(r), 500))
            .filter(|r| !r.is_empty())
            .take(3)
            .map(Value::String)
            .collect();
        Some(out)
    }

    pub fn load_review_context(&self) -> Map<String, Value> {
        let Ok(txt) = read_text(&self.review_context_file()) else { return Map::new() };
        let Some(Value::Object(m)) = js::json_parse(&txt) else { return Map::new() };
        let mut clean = Map::new();
        for (asin, reviews) in m {
            if !notes::context_asin_ok(&asin) {
                continue;
            }
            if let Some(valid) = Self::clean_reviews(&reviews) {
                if !valid.is_empty() {
                    clean.insert(asin, Value::Array(valid));
                }
            }
        }
        clean
    }

    pub fn update_review_context(&self, items: &[Value]) -> Map<String, Value> {
        let mut updates: Vec<(String, Value)> = Vec::new();
        for item in items {
            let asin = to_str_or_empty(prop(item, "asin"));
            if !notes::context_asin_ok(&asin) {
                continue;
            }
            let Some(tr) = prop(item, "top_reviews") else { continue };
            let Some(reviews) = Self::clean_reviews(tr) else { continue };
            if !reviews.is_empty() {
                if let Some(e) = updates.iter_mut().find(|(a, _)| *a == asin) {
                    e.1 = Value::Array(reviews);
                } else {
                    updates.push((asin, Value::Array(reviews)));
                }
            }
        }
        let mut merged = self.load_review_context();
        if updates.is_empty() {
            return merged;
        }
        for (a, r) in updates {
            merged.insert(a, r);
        }
        js::order_object(&mut merged);
        let body = js::stringify_pretty(&Value::Object(merged.clone())) + "\n";
        if let Err(e) = write_atomic(&self.review_context_file(), body.as_bytes(), false) {
            elog!("Failed to persist review context: {}", e);
        }
        merged
    }

    // ------------------------------------------------------------------
    // Price-drop watch sidecar
    // ------------------------------------------------------------------

    fn price_history_file(&self) -> PathBuf {
        self.notes_path.join(".price-history.json")
    }

    pub fn load_price_history(&self) -> Map<String, Value> {
        let Ok(txt) = read_text(&self.price_history_file()) else { return Map::new() };
        let Some(Value::Object(m)) = js::json_parse(&txt) else { return Map::new() };
        let mut clean = Map::new();
        for (asin, entry) in m {
            if !notes::context_asin_ok(&asin) {
                continue;
            }
            let Value::Object(e) = &entry else { continue };
            let Some(paid) = js::json_num(e.get("paid")).filter(|p| p.is_finite()) else { continue };
            let hist: Vec<Value> = e
                .get("history")
                .and_then(|h| h.as_array())
                .map(|a| {
                    a.iter()
                        .filter(|h| {
                            js::json_num(h.get("t")).map(f64::is_finite).unwrap_or(false)
                                && js::json_num(h.get("price")).map(f64::is_finite).unwrap_or(false)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let skip = hist.len().saturating_sub(PRICE_HISTORY_CAP);
            let hist: Vec<Value> = hist[skip..].iter().map(|h| json!({ "t": h["t"], "price": h["price"] })).collect();
            clean.insert(asin, json!({ "paid": js::jnum(paid), "history": hist }));
        }
        clean
    }

    pub fn update_price_history(&self, items: &[Value]) -> Result<(), String> {
        let mut updates: Vec<(String, f64, f64)> = Vec::new();
        for item in items {
            let asin = to_str_or_empty(prop(item, "asin"));
            if !notes::context_asin_ok(&asin) {
                continue;
            }
            let listing = notes::parse_price_number(prop(item, "listing_price"));
            let paid = notes::parse_price_number(prop(item, "current_price"));
            let (Some(listing), Some(paid)) = (listing, paid) else { continue };
            if let Some(e) = updates.iter_mut().find(|(a, _, _)| *a == asin) {
                e.1 = paid;
                e.2 = listing;
            } else {
                updates.push((asin, paid, listing));
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let mut history = self.load_price_history();
        let now = jsdate::now_ms() as f64;
        for (asin, paid, listing) in updates {
            let mut entry = history.get(&asin).cloned().unwrap_or_else(|| json!({ "paid": js::jnum(paid), "history": [] }));
            entry["paid"] = js::jnum(paid);
            let hist = entry["history"].as_array_mut().unwrap();
            let same_day = hist.last().and_then(|l| l["t"].as_f64()).map(|t| jsdate::same_local_day(t, now)).unwrap_or(false);
            if same_day {
                hist.last_mut().unwrap()["price"] = js::jnum(listing);
            } else {
                hist.push(json!({ "t": js::jnum(now), "price": js::jnum(listing) }));
            }
            while hist.len() > PRICE_HISTORY_CAP {
                hist.remove(0);
            }
            history.insert(asin, entry);
        }
        js::order_object(&mut history);
        let body = js::stringify_pretty(&Value::Object(history)) + "\n";
        write_atomic(&self.price_history_file(), body.as_bytes(), false).map_err(|e| io_msg(&e))
    }

    // ------------------------------------------------------------------
    // Gemini
    // ------------------------------------------------------------------

    pub fn gemini_config(&self) -> gemini::GeminiConfig {
        gemini::GeminiConfig {
            api_key: self.gemini_api_key.clone(),
            notes_path: self.notes_path.clone(),
            daily_limit: self.gemini_daily_limit,
            notifications_enabled: self.notifications_enabled,
            model: self.gemini_model.clone(),
            temperature: self.gemini_temperature,
            style_exemplars: self.style_exemplars,
            endpoint: self.gemini_endpoint.clone(),
        }
    }

    /// Your own posted reviews, most helpful first: voice samples for drafts.
    pub fn style_exemplars_for(&self) -> Vec<(String, String)> {
        if !self.style_exemplars {
            return Vec::new();
        }
        let now = jsdate::now_ms();
        if let Some((at, items)) = self.exemplar_cache.lock().unwrap().as_ref() {
            if now - at < STYLE_EXEMPLAR_TTL_MS {
                return items.clone();
            }
        }
        let orders = self.scan_notes();
        let mut cands: Vec<(String, String, f64, f64)> = Vec::new();
        for o in &orders {
            let modified = js::to_number(o.get("modified"));
            let modified = if modified.is_nan() { 0.0 } else { modified };
            for it in o["items"].as_array().into_iter().flatten() {
                let text = js::trim(it["reviewText"].as_str().unwrap_or("")).to_string();
                let title = js::trim(it["reviewTitle"].as_str().unwrap_or("")).to_string();
                let len = js::utf16_len(&text);
                if it["posted"] != Value::Bool(true) || !(120..=1600).contains(&len) || title.is_empty() {
                    continue;
                }
                if notes::is_template_draft(&format!("**Title:** {}\n**Review:** {}", title, text)) {
                    continue;
                }
                let votes = js::to_number(it.get("helpfulVotes"));
                let votes = if votes.is_nan() { 0.0 } else { votes };
                cands.push((title, text, votes, modified));
            }
        }
        cands.sort_by(|a, b| {
            let d = b.2 - a.2;
            let d = if d != 0.0 { d } else { b.3 - a.3 };
            d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        let items: Vec<(String, String)> = cands.into_iter().take(STYLE_EXEMPLAR_POOL).map(|(t, x, _, _)| (t, x)).collect();
        *self.exemplar_cache.lock().unwrap() = if items.is_empty() { None } else { Some((now, items.clone())) };
        items
    }

    pub fn generate_review(&self, item: &ReviewItem, rating: f64, top_reviews: &[String], tone: &str, shape_seed: Option<u32>) -> Draft {
        let cfg = self.gemini_config();
        gemini::generate_review(&cfg, item, rating, top_reviews, tone, shape_seed, &|| self.style_exemplars_for())
    }

    // ------------------------------------------------------------------
    // Sync
    // ------------------------------------------------------------------

    pub fn process_payload(&self, payload: &mut Value) -> Result<(), String> {
        if self.notifications_enabled {
            notify(&format!("Syncing order {}...", to_str(prop(payload, "order_id"))));
        }
        self.create_or_update_order(payload, true)?;
        let items: Vec<Value> = prop(payload, "items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if let Err(e) = self.update_price_history(&items) {
            elog!("Failed to update price history: {}", e);
        }
        if self.notifications_enabled {
            notify(&format!("Order {} synced!", to_str(prop(payload, "order_id"))));
        }
        Ok(())
    }

    pub fn create_or_update_order(&self, payload: &mut Value, generate_drafts: bool) -> Result<(), String> {
        match payload {
            Value::Null => return Err("Cannot set properties of null (setting 'items')".into()),
            Value::Object(m) => {
                if !matches!(m.get("items"), Some(Value::Array(_))) {
                    m.insert("items".into(), Value::Array(Vec::new()));
                }
                let items = m["items"].as_array().cloned().unwrap_or_default();
                let deduped = notes::dedupe_items_by_asin(&items, m.get("order_id"));
                m.insert("items".into(), Value::Array(deduped));
            }
            Value::Array(_) => {}
            // Strict-mode JS: `payload.items = []` on a primitive throws.
            other => {
                let kind = match other {
                    Value::Bool(_) => "boolean",
                    Value::Number(_) => "number",
                    _ => "string",
                };
                return Err(format!("Cannot create property 'items' on {} '{}'", kind, to_str(Some(other))));
            }
        }
        let order_id = notes::sanitize_id(prop(payload, "order_id"));
        if !notes::is_safe_id(&order_id) {
            return Err("Invalid order_id".into());
        }
        let items: Vec<Value> = prop(payload, "items").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let mut folder = "Orders".to_string();
        if js_truthy(prop(payload, "date")) {
            if let Some(t) = jsdate::parse(&to_str(prop(payload, "date")).replacen("Ordered on ", "", 1)) {
                let p = jsdate::local_parts(t);
                folder = format!("Orders/{}/{:02}-{}", p.year, p.month0 + 1, jsdate::MONTH_NAMES[p.month0 as usize]);
            }
        }
        self.ensure_folder_exists(&folder);
        let file_path = self.vault_path(&folder).join(format!("Order-{}.md", order_id));

        let existing_file = self.find_order_file(&order_id);
        let existing = match &existing_file {
            Some(f) => read_text(f).map_err(|e| io_msg(&e))?,
            None => String::new(),
        };

        let context = self.update_review_context(&items);
        let mut extras = Vec::new();
        let blocks: Vec<String> = if existing.is_empty() { Vec::new() } else { js::split(js::re(r"\r?\n---\r?\n", ""), &existing) };
        let fm_asins = if existing.is_empty() { Vec::new() } else { notes::parse_array_field(&existing, "asin") };

        for item in &items {
            if item.is_null() {
                return Err("Cannot read properties of null (reading 'image')".into());
            }
            let image = self.download_image(prop(item, "image"), prop(item, "asin"));
            let asin_s = to_str(prop(item, "asin"));
            let mut ai_review = String::new();
            let mut template_review = String::new();
            let mut already_posted = false;

            if !existing.is_empty() {
                if let Some(block) = blocks.iter().find(|b| b.contains(&asin_s) && b.contains(notes::REVIEW_HEADER)) {
                    if let Some(own_raw) = notes::existing_review(block) {
                        let own = notes::without_placeholder(&own_raw);
                        if notes::has_review_content(&own) {
                            if !self.gemini_api_key.is_empty() && notes::is_template_draft(&own) {
                                template_review = own;
                            } else {
                                ai_review = own;
                            }
                        }
                    }
                    if notes::is_posted_block(block) {
                        already_posted = true;
                    }
                }
            }

            let title_ok = js_truthy(prop(item, "review_title")) && prop(item, "review_title") != Some(&json!("Unknown"));
            let text_ok = js_truthy(prop(item, "review_text")) && prop(item, "review_text") != Some(&json!("Unknown"));
            let has_imported = js_truthy(prop(item, "is_reviewed")) && title_ok && text_ok;
            let is_posted = has_imported || already_posted;

            // Index of this item's ASIN in the note (strict equality: only a
            // string ASIN can match).
            let asin_index = match prop(item, "asin") {
                Some(Value::String(a)) if !existing.is_empty() => fm_asins.iter().position(|x| x == a),
                _ => None,
            };

            let mut final_rating: Option<Value> = prop(item, "review_rating").cloned();
            if !js_truthy(final_rating.as_ref()) && !existing.is_empty() {
                let ratings = notes::parse_aligned_array(&existing, "rating");
                if let Some(i) = asin_index {
                    if let Some(r) = ratings.get(i).filter(|r| !r.is_empty()) {
                        final_rating = Some(Value::String(r.clone()));
                    }
                }
            }

            let mut final_votes = notes::non_negative_int(prop(item, "helpful_votes"));
            if final_votes.is_none() && !existing.is_empty() {
                let votes = notes::parse_aligned_array(&existing, "helpful_votes");
                if let Some(i) = asin_index {
                    final_votes = notes::non_negative_int_str(votes.get(i).map(|s| s.as_str()));
                }
            }

            let mut final_tone = String::new();
            if !existing.is_empty() {
                let tones = notes::parse_aligned_array(&existing, "tone");
                if let Some(i) = asin_index {
                    if let Some(t) = tones.get(i).filter(|t| !t.is_empty()) {
                        final_tone = t.clone();
                    }
                }
            }

            if !is_posted && ai_review.is_empty() && !self.gemini_api_key.is_empty() && generate_drafts {
                let asin_key = to_str_or_empty(prop(item, "asin"));
                let auto_rating = notes::default_rating_for_asin(&asin_key);
                let top: Vec<String> = context
                    .get(&asin_key)
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let draft = self.generate_review(&ReviewItem::from_payload(item), auto_rating as f64, &top, "balanced", None);
                if js_truthy(Some(&draft.text)) {
                    ai_review = format!(
                        "**Rating:** {}\n**Title:** {}\n**Review:** {}",
                        auto_rating,
                        to_str(Some(&draft.title)),
                        to_str(Some(&draft.text))
                    );
                }
            }
            if ai_review.is_empty() && !template_review.is_empty() {
                ai_review = template_review;
            }

            let review = if has_imported {
                let t = js::replace_str(
                    js::re(r"\r?\n", "g"),
                    &notes::safe_review_text(&to_str_or_empty(prop(item, "review_title"))),
                    true,
                    " ",
                );
                format!("**Title:** {}\n**Review:** {}", t, notes::safe_review_text(&to_str_or_empty(prop(item, "review_text"))))
            } else if !ai_review.is_empty() {
                ai_review
            } else {
                notes::PLACEHOLDER.to_string()
            };

            extras.push(ItemExtra {
                image,
                review: Some(review),
                posted: is_posted,
                rating: Some(match final_rating {
                    None | Some(Value::Null) => Value::String(String::new()),
                    Some(v) => v,
                }),
                helpful_votes: final_votes,
                tone: final_tone,
            });
        }

        let content = notes::build_note(payload, &extras);
        let mut target = file_path.clone();
        if let Some(existing_file) = existing_file {
            if existing_file != file_path {
                if let Err(e) = std::fs::rename(&existing_file, &file_path) {
                    elog!("Failed to move file to new folder: {}", e);
                    target = existing_file;
                }
            }
        }
        self.write_markdown_atomic(&target, &content).map_err(|e| io_msg(&e))
    }

    // ------------------------------------------------------------------
    // Scan (per-file parse cache)
    // ------------------------------------------------------------------

    pub fn invalidate_scan_cache(&self) {
        self.scan.lock().unwrap().files.clear();
    }

    #[cfg(test)]
    pub fn last_scan_stats(&self) -> (usize, usize) {
        let s = self.scan.lock().unwrap();
        (s.walked, s.reparsed)
    }

    pub fn asset_images(&self) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        let Ok(entries) = read_dir_sorted(&self.notes_path.join("Orders").join("assets")) else { return map };
        for e in entries {
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(dot) = name.rfind('.') else { continue };
            if dot == 0 {
                continue;
            }
            let ext = name[dot + 1..].to_lowercase();
            if asset_type(&ext).is_none() {
                continue;
            }
            map.entry(name[..dot].to_string()).or_insert_with(|| format!("assets/{}", name));
        }
        map
    }

    fn refresh_scan(&self, cache: &mut ScanCache) {
        let files: Vec<PathBuf> = self
            .walk_orders()
            .into_iter()
            .filter(|f| {
                let n = file_name_str(f);
                n.starts_with("Order-") && f.to_string_lossy().ends_with(".md")
            })
            .collect();
        let images = self.asset_images();
        let image_for = |asin: &str| images.get(&notes::sanitize_id_str(asin)).cloned();
        let mut seen = std::collections::HashSet::new();
        let mut reparsed = 0;
        for file in &files {
            seen.insert(file.clone());
            let Ok(md) = std::fs::metadata(file) else { continue };
            let mt = mtime_ms(&md);
            if let Some(c) = cache.files.get(file) {
                if c.mtime_ms == mt && c.size == md.len() {
                    continue;
                }
            }
            let Ok(content) = read_text(file) else {
                cache.files.shift_remove(file);
                continue;
            };
            let rel = file
                .strip_prefix(&self.notes_path)
                .map(|p| p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
                .unwrap_or_default();
            match notes::parse_note(&content, &rel, js::math_round(mt), &image_for) {
                Some(p) => {
                    let entry = CacheEntry { mtime_ms: mt, size: md.len(), sort_key: p.sort_key, parsed: p.order };
                    cache.files.insert(file.clone(), entry);
                }
                None => {
                    cache.files.shift_remove(file);
                }
            }
            reparsed += 1;
        }
        cache.files.retain(|k, _| seen.contains(k));
        cache.walked = files.len();
        cache.reparsed = reparsed;
    }

    /// Parsed orders, newest first.
    pub fn scan_notes(&self) -> Vec<Value> {
        self.scan_notes_mut(|_| {})
    }

    /// Scan, then let `f` annotate the cached order objects in place (the
    /// /api/orders route adds fields that later scans keep, as in the Node
    /// server), and return copies.
    pub fn scan_notes_mut(&self, f: impl FnOnce(&mut Vec<&mut Value>)) -> Vec<Value> {
        let mut cache = self.scan.lock().unwrap();
        self.refresh_scan(&mut cache);
        let mut entries: Vec<&mut CacheEntry> = cache.files.values_mut().collect();
        entries.sort_by(|a, b| {
            let d = b.sort_key - a.sort_key;
            d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut orders: Vec<&mut Value> = entries.into_iter().map(|e| &mut e.parsed).collect();
        f(&mut orders);
        orders.into_iter().map(|v| v.clone()).collect()
    }

    /// The most recently modified note whose frontmatter lists the ASIN.
    pub fn find_latest_note_with_asin(&self, asin: &str) -> Option<(PathBuf, String)> {
        let mut best: Option<(PathBuf, String, f64)> = None;
        for file in self.walk_orders() {
            if !file.to_string_lossy().ends_with(".md") {
                continue;
            }
            let Ok(content) = read_text(&file) else { continue };
            let Ok(md) = std::fs::metadata(&file) else { continue };
            let Some(fm) = notes::frontmatter(&content) else { continue };
            if !notes::parse_array_field(&fm, "asin").iter().any(|a| a == asin) {
                continue;
            }
            let mt = mtime_ms(&md);
            if best.as_ref().map(|b| mt > b.2).unwrap_or(true) {
                best = Some((file, content, mt));
            }
        }
        best.map(|(f, c, _)| (f, c))
    }
}

/// JS truthiness of a JSON value (None = undefined).
pub fn js_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// An HTTP client with an overall timeout (system TLS on Windows).
pub fn agent(timeout: Duration) -> ureq::Agent {
    let b = ureq::AgentBuilder::new().timeout(timeout);
    #[cfg(windows)]
    let b = match native_tls::TlsConnector::new() {
        Ok(c) => b.tls_connector(std::sync::Arc::new(c)),
        Err(_) => b,
    };
    b.build()
}

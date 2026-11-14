//! Tests of the server's internals: the note format, the store, Gemini, the
//! headless runner. They were in-process tests of server.js
//! (tests/server.test.js) and keep their numbers; the HTTP behaviour is
//! tested against the built program by tests/server.test.js.

use crate::backfill;
use crate::config::{self, Config};
use crate::gemini::{self, Draft, Endpoint, GeminiConfig, ReviewItem};
use crate::headless::{self, Headless, RunResult};
use crate::http;
use crate::js;
use crate::jsdate;
use crate::logbuf;
use crate::notes;
use crate::store::{self, Store};
use serde_json::{json, Value};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Tests that touch process-wide state (logs, quota marks, browser cache,
/// PATH) take this lock.
static GLOBAL: Mutex<()> = Mutex::new(());

fn global() -> MutexGuard<'static, ()> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("aos-rs-{}-{}-{}", tag, std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

enum Reply {
    Bytes(u16, Vec<(&'static str, String)>, Vec<u8>),
    Chunked(u16, &'static str, Vec<u8>),
    Hang(Duration),
}

fn json_reply(status: u16, body: Value) -> Reply {
    Reply::Bytes(status, vec![("Content-Type", "application/json".into())], js::stringify(&body).into_bytes())
}

fn draft_reply(title: &str, text: &str) -> Reply {
    json_reply(
        200,
        json!({ "candidates": [{ "content": { "parts": [{ "text": js::stringify(&json!({ "title": title, "text": text })) }] } }] }),
    )
}

/// A local HTTP server: `handler(n, url, body)` answers the n-th request.
struct Mock {
    url: String,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    server: Arc<tiny_http::Server>,
}

impl Mock {
    fn new(handler: impl Fn(usize, &str, &str) -> Reply + Send + Sync + 'static) -> Mock {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (s, c, h) = (Arc::clone(&server), Arc::clone(&calls), Arc::new(handler));
        std::thread::spawn(move || {
            for mut req in s.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let n = {
                    let mut c = c.lock().unwrap();
                    c.push((req.url().to_string(), body.clone()));
                    c.len() - 1
                };
                let reply = h(n, req.url(), &body);
                std::thread::spawn(move || match reply {
                    Reply::Bytes(status, headers, data) => {
                        let mut r = tiny_http::Response::from_data(data).with_status_code(status);
                        for (k, v) in headers {
                            r.add_header(tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap());
                        }
                        let _ = req.respond(r);
                    }
                    Reply::Chunked(status, ctype, data) => {
                        let h = tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
                        let r = tiny_http::Response::new(tiny_http::StatusCode(status), vec![h], std::io::Cursor::new(data), None, None);
                        let _ = req.respond(r);
                    }
                    Reply::Hang(d) => {
                        std::thread::sleep(d);
                        drop(req);
                    }
                });
            }
        });
        Mock { url, calls, server }
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

fn endpoint(base: &str) -> Endpoint {
    Endpoint { base_url: base.to_string(), timeout_ms: 300, wait_scale: 0.0 }
}

fn gcfg(dir: &Path, base: &str, limit: i64) -> GeminiConfig {
    GeminiConfig {
        api_key: "fake-key".into(),
        notes_path: dir.to_path_buf(),
        daily_limit: limit,
        notifications_enabled: false,
        model: config::DEFAULT_GEMINI_MODEL.into(),
        temperature: config::DEFAULT_GEMINI_TEMPERATURE,
        style_exemplars: false,
        endpoint: endpoint(base),
    }
}

fn item(name: &str, asin: Option<&str>) -> ReviewItem {
    ReviewItem { name: Some(json!(name)), asin: asin.map(|a| json!(a)), ..Default::default() }
}

fn gen(cfg: &GeminiConfig, it: &ReviewItem, rating: f64) -> Draft {
    gemini::generate_review(cfg, it, rating, &[], "balanced", None, &Vec::new)
}

fn store_with(dir: &Path, extra: Value) -> Store {
    let mut c = json!({ "notesPath": dir.to_string_lossy(), "geminiApiKey": "", "notificationsEnabled": false });
    for (k, v) in extra.as_object().unwrap() {
        c[k] = v.clone();
    }
    let cfg: Config = config::from_value(PathBuf::new(), &c);
    Store::new(&cfg)
}

fn sync(store: &Store, mut payload: Value) {
    store.process_payload(&mut payload).unwrap();
    store.invalidate_scan_cache();
}

/// A stand-in browser: sleeps (or exits with `exit`), on every OS.
fn fake_browser(dir: &Path, name: &str, exit: Option<i32>) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(format!("{}.sh", name));
        let body = match exit {
            Some(c) => format!("#!/bin/sh\nexit {}\n", c),
            None => "#!/bin/sh\nsleep 60\n".to_string(),
        };
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }
    #[cfg(windows)]
    {
        let p = dir.join(format!("{}.cmd", name));
        let body = match exit {
            Some(c) => format!("@exit /b {}\r\n", c),
            None => "@ping -n 61 127.0.0.1 >nul\r\n".to_string(),
        };
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    }
}

/// Start a run on a thread; wait until it awaits its completion ping.
fn start_run(h: &Arc<Headless>) -> std::thread::JoinHandle<RunResult> {
    let h2 = Arc::clone(h);
    let t = std::thread::spawn(move || h2.run_headless_sync(None));
    for _ in 0..200 {
        if h.running() && h.is_pending() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(h.running() && h.is_pending(), "run should be in progress");
    t
}

fn done(r: RunResult) -> Value {
    match r {
        RunResult::Done(v) | RunResult::NoBrowser(v) => v,
        RunResult::Already => panic!("already running"),
    }
}

fn log_cursor() -> f64 {
    logbuf::read_logs(None)["next"].as_f64().unwrap()
}

fn logs_since(c: f64) -> Vec<String> {
    logbuf::read_logs(Some(c))["lines"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap().to_string()).collect()
}

fn order(id: &str, items: Value) -> Value {
    json!({ "order_id": id, "date": "August 1, 2026", "total": "$9.99", "status": "Delivered", "ship_to": "Tester", "items": items })
}

fn widget(asin: &str) -> Value {
    json!({ "asin": asin, "name": format!("Widget {}", asin), "brand": "Acme", "current_price": "$9.99",
        "link": format!("https://www.amazon.com/dp/{}", asin), "features": "- feature", "image": "" })
}

fn note_text(store: &Store, id: &str) -> String {
    store::read_text(&store.find_order_file(id).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------
// The note format
// ---------------------------------------------------------------------------

#[test]
fn t01_parse_array_field_forms() {
    assert_eq!(notes::parse_array_field(r#"brand: ["Acme, Inc.", "Foo, Bar & Co"]"#, "brand"), vec!["Acme, Inc.", "Foo, Bar & Co"]);
    assert_eq!(notes::parse_array_field("asin:\n  - \"AAA\"\n  - BBB\n", "asin"), vec!["AAA", "BBB"]);
    assert_eq!(notes::parse_array_field("asin: \"ONLYONE\"\n", "asin"), vec!["ONLYONE"]);
}

#[test]
fn t02_build_note_derives_assets_path() {
    let note = notes::build_note(
        &json!({ "order_id": "999-8888888-7777777", "date": "August 1, 2026", "total": "$9.99", "status": "Delivered", "ship_to": "Tester",
            "items": [{ "asin": "B0-ABC123", "name": "Widget", "brand": "Acme", "current_price": "$9.99",
                "link": "https://www.amazon.com/dp/B0-ABC123", "features": "- feature",
                "image": "https://m.media-amazon.com/images/I/abc.jpg?_AC_SL1500_" }] }),
        &[],
    );
    assert!(note.contains("![image](assets/B0-ABC123.jpg)"), "{}", note);
    assert!(note.contains("asin: [\"B0-ABC123\"]"));
}

#[test]
fn t10_default_rating_is_stable_4_or_5() {
    for a in ["B0DEMOAAA1", "B0DEMOBBB2", "B000000000", "B0-ABC123", "X", "B0DEMOCCC3", "B07XYZ9876", "a1", "ZZZZZZZZZZZZ"] {
        let r = notes::default_rating_for_asin(a);
        assert!(r == 4 || r == 5);
        assert_eq!(notes::default_rating_for_asin(a), r);
    }
    let sum: u32 = "B0DEMOAAA1".chars().map(|c| c as u32).sum();
    assert_eq!(notes::default_rating_for_asin("B0DEMOAAA1"), if sum % 10 < 4 { 4 } else { 5 });
}

#[test]
fn t11_t50_parse_price_us_and_eu() {
    let p = notes::parse_price_str;
    assert_eq!(p("$0.00"), Some(0.0));
    assert_eq!(p("$12.99"), Some(12.99));
    assert_eq!(p(""), None);
    assert_eq!(p("abc"), None);
    assert_eq!(p("EUR 24.00"), Some(24.0));
    assert_eq!(p("1.234,56"), Some(1234.56));
    assert_eq!(p("12,99"), Some(12.99));
    assert_eq!(p("$1,234.56"), Some(1234.56));
    assert_eq!(p("1,234"), Some(1234.0));
    assert_eq!(notes::parse_price_number(Some(&json!(12))), None, "only strings parse");
}

#[test]
fn t12_parse_csv() {
    let rows = backfill::parse_csv("Name,Note,Qty\n\"x, y\",\"line1\nline2\",\"he said \"\"hi\"\"\"\nplain,2,3\n");
    assert_eq!(rows.len(), 2);
    let get = |r: &Vec<(String, String)>, k: &str| r.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    assert_eq!(get(&rows[0], "name").as_deref(), Some("x, y"));
    assert_eq!(get(&rows[0], "note").as_deref(), Some("line1\nline2"));
    assert_eq!(get(&rows[0], "qty").as_deref(), Some("he said \"hi\""));
    assert_eq!(get(&rows[1], "note").as_deref(), Some("2"));
    let ci = backfill::parse_csv("Order ID,Product Name\nX1,Thing\n");
    assert_eq!(get(&ci[0], "order id").as_deref(), Some("X1"));
    assert_eq!(get(&ci[0], "asin"), None);
    assert!(backfill::parse_csv("").is_empty());
}

fn two_item_note(fm_extra: &str) -> String {
    format!(
        "---\ntype: order\norder_id: \"111-ALIGN-TEST\"\ndate: \"Ordered on July 4, 2026\"\ntotal: \"$49.98\"\nstatus: \"Delivered\"\nship_to: \"Alex Doe\"\n{}---\n# Order 111-ALIGN-TEST\n\n## 📦 Items\n\n### First Item\n\n#### 📝 My Review\n- [ ] Posted to Amazon?\n\n### Second Item\n\n#### 📝 My Review\n**Rating:** 4\n**Title:** Real review\n**Review:** Body text.\n- [x] Posted to Amazon?\n",
        fm_extra
    )
}

#[test]
fn t31_t40_aligned_arrays_keep_empty_entries() {
    let note = two_item_note("brand: [\"\", \"Sony\"]\nprice: [\"$24.99\", \"$24.99\"]\nasin: [\"B0FIRST0001\", \"B0SECOND001\"]\nurl: [\"\", \"\"]\nreviewed: [false, true]\nrating: [\"\", \"4\"]\nhelpful_votes: [\"\", \"3\"]\ntone: [\"\", \"concise\"]\n");
    let p = notes::parse_note(&note, "", 0.0, &|_| None).unwrap().order;
    let items = p["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["asin"], "B0FIRST0001");
    assert_eq!(items[0]["helpfulVotes"], 0, "item 1 must not inherit item 2's votes");
    assert_eq!(items[0]["tone"], "");
    assert_eq!(items[0]["rating"], "");
    assert_eq!(items[0]["brand"], "");
    assert_eq!(items[1]["helpfulVotes"], 3);
    assert_eq!(items[1]["tone"], "concise");
    assert_eq!(items[1]["rating"], "4");
    assert_eq!(items[1]["brand"], "Sony");
}

#[test]
fn t56_bracket_inside_a_quoted_value() {
    let w = |asin: &str, brand: &str| json!({ "asin": asin, "name": "W", "brand": brand, "current_price": "$10.00", "link": "", "features": "- f", "image": "" });
    let note = notes::build_note(&order("111-2222222-3333333", json!([w("B0BRACKET1", "Foo]Bar"), w("B0BRACKET2", "Acme")])), &[]);
    assert!(note.contains("brand: [\"Foo]Bar\", \"Acme\"]"));
    let p = notes::parse_note(&note, "", 0.0, &|_| None).unwrap().order;
    assert_eq!(p["items"][0]["brand"], "Foo]Bar");
    assert_eq!(p["items"][1]["brand"], "Acme");
    assert_eq!(notes::parse_array_field(&note, "brand"), vec!["Foo]Bar", "Acme"]);
    assert_eq!(notes::parse_array_field(&note, "asin"), vec!["B0BRACKET1", "B0BRACKET2"]);
}

#[test]
fn t37_guidance() {
    let g = notes::guidance_for("login-required");
    assert_eq!(g["message"], "Your Amazon session expired.");
    assert!(g["steps"].as_array().unwrap().len() >= 3);
    assert_eq!(notes::guidance_for("no-browser")["message"], "No Chromium-based browser found.");
    assert_eq!(notes::guidance_for("timeout — probably not logged in; run setup")["message"], "The sync browser didn't finish in time.");
    assert_eq!(notes::guidance_for("gemini-limit")["message"], "Daily Gemini request budget reached.");
    assert_eq!(notes::guidance_for("some exotic failure"), json!({ "message": "some exotic failure", "steps": [] }));
}

#[test]
fn v220_review_helpers() {
    assert_eq!(
        notes::without_placeholder("> *Write your review for this item below...*\n**Rating:** \n**Review:** hi"),
        "**Rating:** \n**Review:** hi"
    );
    assert!(!notes::has_review_content("**Rating:** \n**Review:** "));
    assert!(notes::has_review_content("**Title:** Fine\n**Review:** "));
    assert_eq!(notes::safe_review_text("a\n  ----  \nb"), "a\n- - -\nb");
    let items = [json!({ "asin": "A" }), json!({ "asin": "A" }), json!({ "asin": "" }), json!({ "asin": "" })];
    assert_eq!(notes::dedupe_items_by_asin(&items, Some(&json!("x"))).len(), 3);
}

#[test]
fn t86_review_shapes_rotate_by_seed() {
    assert!(notes::REVIEW_SHAPES.len() >= 5);
    assert_eq!(notes::seed_from_string("B0AAAAAAA1"), notes::seed_from_string("B0AAAAAAA1"));
    let shapes: std::collections::HashSet<&str> =
        ["B0AAAAAAA1", "B0BBBBBBB2", "B0CCCCCCC3", "B0DDDDDDD4", "B0EEEEEEE5", "B0FFFFFFF6", "B0GGGGGGG7", "B0HHHHHHH8"]
            .iter()
            .map(|a| notes::shape_for_seed(notes::seed_from_string(a)))
            .collect();
    assert!(shapes.len() >= 3);
    let pool = ["a", "b", "c", "d"];
    assert_eq!(notes::pick_rotating(&pool, 3, 1), vec!["b", "c", "d"]);
    assert_eq!(notes::pick_rotating(&pool, 3, 3), vec!["d", "a", "b"]);
    assert_eq!(notes::pick_rotating(&["a"], 3, 7), vec!["a"]);
    assert!(notes::pick_rotating::<&str>(&[], 3, 7).is_empty());
}

// ---------------------------------------------------------------------------
// Config, hosts, browsers, notifications
// ---------------------------------------------------------------------------

#[test]
fn t79_config_and_data_dirs() {
    let vars = |k: &str| -> Option<String> {
        match k {
            "APPDATA" => Some(r"C:\Users\m\AppData\Roaming".into()),
            "LOCALAPPDATA" => Some(r"C:\Users\m\AppData\Local".into()),
            "XDG_CONFIG_HOME" => Some("/x/cfg".into()),
            "XDG_DATA_HOME" => Some("/x/data".into()),
            _ => None,
        }
    };
    let home = Path::new("/home/m");
    assert_eq!(
        config::default_config_path_with(true, &vars, home),
        Path::new(r"C:\Users\m\AppData\Roaming").join("amazon-order-sync").join("config.json")
    );
    assert_eq!(config::default_data_dir_with(true, &vars, home), Path::new(r"C:\Users\m\AppData\Local").join("amazon-order-sync"));
    assert_eq!(config::default_config_path_with(false, &vars, home), Path::new("/x/cfg").join("amazon-order-sync").join("config.json"));
    assert_eq!(config::default_data_dir_with(false, &vars, home), Path::new("/x/data").join("amazon-order-sync"));
    let none = |_: &str| None;
    assert!(config::default_config_path_with(true, &none, home).starts_with(home));
    assert!(config::default_data_dir_with(false, &none, home).starts_with(home));
}

#[test]
fn t25b_hosts() {
    assert!(http::is_allowed_host("localhost:4000", 4000));
    assert!(http::is_allowed_host("127.0.0.1:4000", 4000));
    assert!(!http::is_allowed_host("rebind.example:4000", 4000));
    assert!(!http::is_allowed_host("localhost:4001", 4000));
    assert!(!http::is_allowed_host("", 4000));
    assert!(http::is_allowed_origin(&format!("chrome-extension://{}", http::EXTENSION_ID), 4000));
    assert!(http::is_allowed_origin("http://localhost:4000", 4000));
    assert!(!http::is_allowed_origin("https://www.amazon.com", 4000));
    assert!(http::is_allowed_origin("", 4000));
}

#[test]
fn t19_t80_browser_autodetect() {
    let t = TempDir::new("browsers");
    let fake = fake_browser(t.path(), "fake", None);
    let none = |_: &str| None;
    let empty = std::ffi::OsStr::new("");
    assert_eq!(headless::find_browser_binary_with(&fake, empty, false, &none), Some(PathBuf::from(&fake)));
    assert_eq!(headless::find_browser_binary_with(&t.path().join("no-such-browser").to_string_lossy(), empty, false, &none), None);
    assert_eq!(headless::find_browser_binary_with("", empty, false, &none), None);

    let local = t.path().join("Local");
    let pf = t.path().join("ProgramFiles");
    let on_path = t.path().join("bin");
    std::fs::create_dir_all(local.join("BraveSoftware/Brave-Browser/Application")).unwrap();
    std::fs::create_dir_all(pf.join("Google/Chrome/Application")).unwrap();
    std::fs::create_dir_all(&on_path).unwrap();
    let brave = local.join("BraveSoftware").join("Brave-Browser").join("Application").join("brave.exe");
    let chrome = pf.join("Google").join("Chrome").join("Application").join("chrome.exe");
    let exe = |p: &Path| {
        std::fs::write(p, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    };
    exe(&chrome);
    let (l, p) = (local.to_string_lossy().into_owned(), pf.to_string_lossy().into_owned());
    let env = move |k: &str| -> Option<String> {
        match k {
            "LOCALAPPDATA" => Some(l.clone()),
            "ProgramFiles" => Some(p.clone()),
            _ => None,
        }
    };
    let cands = headless::windows_browser_candidates_with(&env);
    assert_eq!(cands[0], brave, "per-user Brave is probed first");
    assert!(cands.contains(&chrome));
    assert_eq!(headless::find_browser_binary_with("", empty, true, &env), Some(chrome.clone()));
    exe(&brave);
    assert_eq!(headless::find_browser_binary_with("", empty, true, &env), Some(brave.clone()));
    let path_chrome = on_path.join("chrome.exe");
    exe(&path_chrome);
    assert_eq!(headless::find_browser_binary_with("", on_path.as_os_str(), true, &env), Some(path_chrome));
    assert_eq!(headless::find_browser_binary_with("", empty, false, &env), None, "non-Windows never walks the install folders");
}

#[test]
fn t81_notification_buffer() {
    let _g = global();
    let start = logbuf::read_notifications(None)["next"].as_f64().unwrap();
    logbuf::push_notification("T1", "B1");
    logbuf::push_notification("T2", "B2");
    let fresh = logbuf::read_notifications(None);
    assert_eq!(fresh["items"], json!([]));
    assert_eq!(fresh["next"].as_f64().unwrap(), start + 2.0);
    let since = logbuf::read_notifications(Some(start));
    let titles: Vec<&str> = since["items"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert_eq!(titles, vec!["T1", "T2"]);
    assert_eq!(logbuf::read_notifications(Some(start + 1.0))["items"].as_array().unwrap().len(), 1);
    assert_eq!(logbuf::read_notifications(Some(start + 2.0))["items"], json!([]));
}

#[test]
fn t22_due_predicate() {
    let now = 1_800_000_000_000.0;
    let day = 86_400_000.0;
    assert!(headless::is_due(true, true, false, false, Some(now - 2.0 * day), day, now));
    assert!(!headless::is_due(true, true, false, false, Some(now - 3_600_000.0), day, now));
    assert!(headless::is_due(true, true, false, false, None, day, now));
    assert!(!headless::is_due(false, true, false, false, None, day, now));
    assert!(!headless::is_due(true, false, false, false, None, day, now));
    assert!(!headless::is_due(true, true, true, false, None, day, now));
    assert!(!headless::is_due(true, true, false, true, None, day, now), "a login block stops it");
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

#[test]
fn v220_rename_with_retry() {
    let t = TempDir::new("rename");
    let from = t.path().join("a.tmp");
    std::fs::write(&from, "x").unwrap();
    store::rename_with_retry(&from, &t.path().join("a.md"), 8).unwrap();
    assert_eq!(std::fs::read_to_string(t.path().join("a.md")).unwrap(), "x");
    let orphan = t.path().join("b.tmp");
    std::fs::write(&orphan, "y").unwrap();
    assert!(store::rename_with_retry(&orphan, &t.path().join("missing-dir").join("b.md"), 8).is_err());
    assert!(!orphan.exists(), "the temp file is removed after a failed rename");
}

#[test]
fn v220_backfill_dedupes_repeated_asins() {
    let t = TempDir::new("dedupe");
    let store = store_with(t.path(), json!({}));
    let mut p = json!({ "order_id": "222-0000000-0000001", "date": "2025-01-02", "items": [{ "asin": "B0DUPDUP01", "name": "Thing" }, { "asin": "B0DUPDUP01", "name": "Thing" }] });
    store.create_or_update_order(&mut p, false).unwrap();
    let md = std::fs::read_to_string(t.path().join("Orders/2025/01-January/Order-222-0000000-0000001.md")).unwrap();
    assert!(md.lines().any(|l| l == "asin: [\"B0DUPDUP01\"]"), "{}", md);
}

#[test]
fn v220_typed_review_survives_resync() {
    let t = TempDir::new("typed");
    let store = store_with(t.path(), json!({}));
    let payload = || {
        json!({ "order_id": "111-2223334-5556667", "date": "Ordered on March 3, 2026", "total": "$10", "status": "Delivered", "ship_to": "Alex Doe",
        "items": [{ "asin": "B0V220AAA1", "name": "Widget", "brand": "Acme", "current_price": "$5", "link": "", "features": "- f", "image": "" }] })
    };
    store.create_or_update_order(&mut payload(), true).unwrap();
    let file = t.path().join("Orders/2026/03-March/Order-111-2223334-5556667.md");
    let md = std::fs::read_to_string(&file)
        .unwrap()
        .replace("**Review:** \n- [ ] Posted to Amazon?", "**Review:** My own words about the widget.\n- [ ] Posted to Amazon?");
    std::fs::write(&file, &md).unwrap();
    assert_eq!(notes::parse_note(&md, "", 0.0, &|_| None).unwrap().order["items"][0]["hasDraft"], true);
    store.create_or_update_order(&mut payload(), true).unwrap();
    let md = std::fs::read_to_string(&file).unwrap();
    assert!(md.contains("My own words about the widget."));
    assert!(!md.contains("Write your review for this item below"), "the placeholder goes once there is text");
    let saved = notes::update_review_block(&md, "B0V220AAA1", "Good", "Pros: sturdy\n---\nCons: loud", None).unwrap();
    assert!(saved.contains("Pros: sturdy\n- - -\nCons: loud"));
    std::fs::write(&file, &saved).unwrap();
    store.create_or_update_order(&mut payload(), true).unwrap();
    assert!(std::fs::read_to_string(&file).unwrap().contains("Cons: loud"));
}

#[test]
fn t69_incremental_scan_cache() {
    let t = TempDir::new("inc");
    let store = store_with(t.path(), json!({}));
    for (id, asin, reviewed) in [
        ("111-1000000-0000001", "B0INCACHE1", true),
        ("111-1000000-0000002", "B0INCACHE2", true),
        ("111-1000000-0000003", "B0INCACHE3", false),
    ] {
        let mut w = widget(asin);
        if reviewed {
            w["is_reviewed"] = json!(true);
            w["review_title"] = json!("Original title");
            w["review_text"] = json!("Original body");
            w["review_rating"] = json!("5");
        }
        let mut p = order(id, json!([w]));
        store.process_payload(&mut p).unwrap();
    }
    assert_eq!(store.scan_notes().len(), 3);
    assert_eq!(store.last_scan_stats(), (3, 3));
    assert_eq!(store.scan_notes().len(), 3);
    assert_eq!(store.last_scan_stats(), (3, 0), "steady state re-parses nothing");
    let edited = t.path().join("Orders/2026/08-August/Order-111-1000000-0000002.md");
    let md = std::fs::read_to_string(&edited).unwrap();
    std::fs::write(&edited, md.replace("Original body", "Edited body, now longer")).unwrap();
    let third = store.scan_notes();
    assert_eq!(store.last_scan_stats(), (3, 1));
    let find = |id: &str| third.iter().find(|o| o["orderId"] == id).unwrap().clone();
    assert_eq!(find("111-1000000-0000002")["items"][0]["reviewText"], "Edited body, now longer");
    assert_eq!(find("111-1000000-0000001")["items"][0]["reviewText"], "Original body");
    std::fs::remove_file(t.path().join("Orders/2026/08-August/Order-111-1000000-0000003.md")).unwrap();
    let fourth = store.scan_notes();
    assert_eq!(fourth.len(), 2);
    assert_eq!(store.last_scan_stats(), (2, 0));
}

#[test]
fn t50_prune_vote_timestamps() {
    let t = TempDir::new("prune");
    let store = store_with(t.path(), json!({}));
    sync(&store, order("prune-order-1", json!([widget("B0PRUNE001")])));
    let h = Headless::new_for_tests(t.path().to_path_buf(), false, "", false, t.path().join("profile"));
    h.bump_vote_timestamps(&["B0PRUNE001".into(), "B0GONE0001".into()]);
    http::prune_vote_timestamps(&store, &h);
    let st = h.state();
    assert_eq!(st.vote_timestamps.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), vec!["B0PRUNE001"]);
    let on_disk = js::json_parse(&std::fs::read_to_string(t.path().join(".headless-sync.json")).unwrap()).unwrap();
    assert_eq!(on_disk["voteTimestamps"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["B0PRUNE001"]);
}

#[test]
fn t49_t58_download_image_guards() {
    let big = vec![1u8; 6 * 1024 * 1024];
    let png = vec![0x89u8, 0x50, 0x4e, 0x47, 1, 2, 3, 4];
    let png2 = png.clone();
    let mock = Mock::new(move |_, url, _| {
        let b = |ct: &str, d: Vec<u8>| Reply::Bytes(200, vec![("Content-Type", ct.to_string())], d);
        if url.starts_with("/html") {
            b("text/html", b"<html>not an image</html>".to_vec())
        } else if url.starts_with("/svg") {
            b("image/svg+xml", b"<svg/>".to_vec())
        } else if url.starts_with("/big") {
            b("image/png", big.clone())
        } else if url.starts_with("/stream") {
            Reply::Chunked(200, "image/jpeg", big.clone())
        } else if url.starts_with("/declared") {
            Reply::Bytes(200, vec![("Content-Type", "image/png".into()), ("X-Declared", "6291456".into())], png2.clone())
        } else {
            b("image/png; charset=binary", png2.clone())
        }
    });
    let t = TempDir::new("img");
    let store = store_with(t.path(), json!({}));
    let dl = |u: &str, asin: &str| store.download_image(Some(&json!(u)), Some(&json!(asin)));
    assert_eq!(dl("", "B0IMG0001X"), Some(json!("")));
    assert_eq!(dl("ftp://example.com/x.png", "B0IMG0001X"), Some(json!("ftp://example.com/x.png")));
    assert_eq!(store.download_image(None, Some(&json!("B0IMG0001X"))), None, "undefined stays undefined");
    assert_eq!(dl(&format!("{}/html/evil.jpg", mock.url), "B0IMGHTML1"), Some(Value::Null));
    assert_eq!(dl(&format!("{}/svg/x.svg", mock.url), "B0IMGSVG01"), Some(Value::Null));
    assert_eq!(dl(&format!("{}/big/big.jpg", mock.url), "B0IMGBIG01"), Some(Value::Null));
    assert_eq!(dl(&format!("{}/stream/s.jpg", mock.url), "B0IMGSTRM1"), Some(Value::Null));
    let assets = t.path().join("Orders").join("assets");
    assert_eq!(std::fs::read_dir(&assets).unwrap().count(), 0, "skipped downloads write nothing");
    assert_eq!(dl(&format!("{}/ok.jpg", mock.url), "B0IMGPNG01"), Some(json!("assets/B0IMGPNG01.png")));
    assert_eq!(std::fs::read(assets.join("B0IMGPNG01.png")).unwrap(), png);
    let before = mock.count();
    assert_eq!(dl(&format!("{}/ok.jpg", mock.url), "B0IMGPNG01"), Some(json!("assets/B0IMGPNG01.png")));
    assert_eq!(mock.count(), before, "a cached image is not fetched again");
}

#[test]
fn t58_declared_length_over_the_cap_is_skipped() {
    // A server that declares 6 MB but sends a small body: refused up front.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = server.accept() {
            use std::io::Write;
            let mut buf = [0u8; 2048];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 6291456\r\n\r\n12345678");
        }
    });
    let t = TempDir::new("img58");
    let store = store_with(t.path(), json!({}));
    let r = store.download_image(Some(&json!(format!("http://127.0.0.1:{}/huge.jpg", port))), Some(&json!("B0IMGCL001")));
    assert_eq!(r, Some(Value::Null));
    assert_eq!(std::fs::read_dir(t.path().join("Orders").join("assets")).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

fn today() -> String {
    jsdate::local_date_key(jsdate::now_ms())
}

#[test]
fn t35_quota_persists_rolls_over_and_limits() {
    let _g = global();
    let t = TempDir::new("quota");
    let usage = t.path().join(".gemini-usage.json");
    gemini::record_attempt(t.path()).unwrap();
    let u = gemini::record_attempt(t.path()).unwrap();
    assert_eq!((u.used, u.day.clone()), (2, today()));
    assert_eq!(js::json_parse(&std::fs::read_to_string(&usage).unwrap()).unwrap(), json!({ "day": today(), "used": 2 }));
    std::fs::write(&usage, "{\"day\":\"2000-01-01\",\"used\":42}\n").unwrap();
    assert_eq!(gemini::load_usage(t.path()).used, 0, "a stale day resets");

    let never = Mock::new(|_, _, _| panic!("no request may be made at the limit"));
    std::fs::write(&usage, format!("{{\"day\":\"{}\",\"used\":5}}\n", today())).unwrap();
    let d = gen(&gcfg(t.path(), &never.url, 5), &item("Widget", None), 4.0);
    assert_eq!(d.title, json!("Good overall, one small gripe"));
    assert!(d.fallback);
    assert_eq!(gemini::load_usage(t.path()).used, 5, "a skipped call is not counted");

    let ok = Mock::new(|_, _, _| draft_reply("T", "Body"));
    std::fs::write(&usage, format!("{{\"day\":\"{}\",\"used\":80}}\n", today())).unwrap();
    gemini::reset_quota_log_marks();
    let c = log_cursor();
    let cfg = gcfg(t.path(), &ok.url, 100);
    for _ in 0..2 {
        let d = gen(&cfg, &item("Widget", None), 5.0);
        assert_eq!((d.title, d.text), (json!("T"), json!("Body")));
    }
    let warnings = logs_since(c).iter().filter(|l| l.contains(">=80%")).count();
    assert_eq!(warnings, 1, "one 80% warning per run");
    assert_eq!(gemini::load_usage(t.path()).used, 82, "each real attempt is counted");
}

#[test]
fn t46_429_then_200() {
    let t = TempDir::new("g429");
    let mock = Mock::new(|n, _, _| if n == 0 { json_reply(429, json!({})) } else { draft_reply("Retry Title", "Retry Body") });
    let d = gen(&gcfg(t.path(), &mock.url, 10), &item("Widget", None), 5.0);
    assert_eq!((d.title, d.text), (json!("Retry Title"), json!("Retry Body")));
    assert_eq!(mock.count(), 2);
    assert_eq!(gemini::load_usage(t.path()).used, 2, "the 429 and the 200 both count");
}

#[test]
fn t83_503_retries_and_400_fails_fast() {
    let t = TempDir::new("g503");
    let mock = Mock::new(|n, _, _| if n == 0 { json_reply(503, json!("overloaded")) } else { draft_reply("After 503", "Body") });
    let d = gen(&gcfg(t.path(), &mock.url, 10), &item("Widget", None), 5.0);
    assert_eq!(d.title, json!("After 503"));
    assert_eq!(mock.count(), 2);
    let bad = Mock::new(|_, _, _| json_reply(400, json!({ "error": { "message": "API key not valid" } })));
    let d = gen(&gcfg(t.path(), &bad.url, 10), &item("Widget", None), 5.0);
    assert_eq!(d.text, json!(""), "a 400 gives no draft");
    assert_eq!(bad.count(), 1, "a 400 is not retried");
}

#[test]
fn t87_timeout_aborts_and_retries() {
    let t = TempDir::new("gtimeout");
    let mock = Mock::new(|n, _, _| if n == 0 { Reply::Hang(Duration::from_millis(1500)) } else { draft_reply("After hang", "B") });
    let d = gen(&gcfg(t.path(), &mock.url, 10), &item("W", Some("B0TIMEOUT1")), 5.0);
    assert_eq!((d.title, d.text), (json!("After hang"), json!("B")));
    assert_eq!(mock.count(), 2, "the timed-out request is retried once");
}

#[test]
fn t86_shape_reaches_the_prompt() {
    let t = TempDir::new("gshape");
    let mock = Mock::new(|_, _, _| draft_reply("T", "B"));
    let cfg = gcfg(t.path(), &mock.url, 100);
    let prompt = |m: &Mock, i: usize| -> String {
        let body = m.calls.lock().unwrap()[i].1.clone();
        js::json_parse(&body).unwrap()["contents"][0]["parts"][0]["text"].as_str().unwrap().to_string()
    };
    gen(&cfg, &item("W", Some("B0AAAAAAA1")), 5.0);
    let expected = notes::shape_for_seed(notes::seed_from_string("B0AAAAAAA1"));
    let p = prompt(&mock, 0);
    assert!(p.contains("# Shape for THIS review") && p.contains(expected));
    let mut seed = 0;
    while notes::shape_for_seed(seed) == expected {
        seed += 1;
    }
    gemini::generate_review(&cfg, &item("W", Some("B0AAAAAAA1")), 5.0, &[], "balanced", Some(seed), &Vec::new);
    let p = prompt(&mock, 1);
    assert!(p.contains(notes::shape_for_seed(seed)) && !p.contains(expected), "a seed override changes the shape");
}

#[test]
fn t84_resync_retries_over_placeholder_and_template() {
    let t = TempDir::new("resync");
    let mode = Arc::new(Mutex::new(("fail".to_string(), String::new())));
    let m2 = Arc::clone(&mode);
    let mock = Mock::new(move |_, _, _| {
        let m = m2.lock().unwrap();
        if m.0 == "fail" {
            json_reply(400, json!("bad"))
        } else {
            draft_reply(&m.1, &format!("AI body {}", m.1))
        }
    });
    let mut store = store_with(t.path(), json!({ "geminiApiKey": "fake-key", "geminiDailyLimit": 100 }));
    store.gemini_endpoint = endpoint(&mock.url);
    let id = "555-0000000-0000001";
    let payload = || order(id, json!([widget("B0RESYNC01")]));
    let set = |m: &str, title: &str| *mode.lock().unwrap() = (m.to_string(), title.to_string());

    sync(&store, payload());
    assert!(note_text(&store, id).contains("Write your review for this item below"), "a failure leaves the placeholder");
    set("ok", "First");
    sync(&store, payload());
    assert!(note_text(&store, id).contains("**Title:** First"), "re-sync drafted over the placeholder");
    let before = mock.count();
    set("ok", "Second");
    sync(&store, payload());
    assert_eq!(mock.count(), before, "an existing draft is kept without calling Gemini");
    assert!(note_text(&store, id).contains("**Title:** First"));

    let file = store.find_order_file(id).unwrap();
    let template = "**Rating:** 4\n**Title:** Good overall, one small gripe\n**Review:** Happy with it overall: it handles the main job well and feels well made. There is one small thing I would tweak, but it does not change my recommendation.";
    let content = std::fs::read_to_string(&file).unwrap();
    let content =
        js::replace_str(js::re(r"\*\*Rating:\*\* \d\n\*\*Title:\*\* First\n\*\*Review:\*\* AI body First", ""), &content, false, template);
    assert!(content.contains("Good overall, one small gripe"));
    std::fs::write(&file, content).unwrap();
    assert!(notes::is_template_draft(template));
    assert!(!notes::is_template_draft(&format!("{} Also the box was dented.", template)));
    set("fail", "");
    sync(&store, payload());
    assert!(note_text(&store, id).contains("Good overall, one small gripe"), "the template stays when the retry fails");
    set("ok", "Third");
    sync(&store, payload());
    let fin = note_text(&store, id);
    assert!(fin.contains("**Title:** Third"));
    assert!(!fin.contains("Good overall, one small gripe"));
}

#[test]
fn t85_model_temperature_and_style_exemplars() {
    let t = TempDir::new("style");
    let mock = Mock::new(|n, _, _| if n == 0 { draft_reply("Draft T", "Draft body") } else { draft_reply("New T", "New body") });
    let mut store = store_with(
        t.path(),
        json!({ "geminiApiKey": "fake-key", "geminiDailyLimit": 100, "geminiModel": "gemini-test-model", "geminiTemperature": 0.4 }),
    );
    store.gemini_endpoint = endpoint(&mock.url);
    let long_body = "This is a posted review body with enough length to qualify as a voice sample. ".repeat(3).trim().to_string();
    let posted = |asin: &str, title: &str, text: &str| {
        let mut w = widget(asin);
        w["is_reviewed"] = json!(true);
        w["review_title"] = json!(title);
        w["review_text"] = json!(text);
        w["review_rating"] = json!("5");
        w
    };
    sync(
        &store,
        order(
            "555-0000000-0000011",
            json!([posted("B0STYLEAA1", "My posted title", &long_body), posted("B0STYLEBB2", "Short", "Too short."), widget("B0STYLECC3")]),
        ),
    );
    assert_eq!(store.style_exemplars_for(), vec![("My posted title".to_string(), long_body.clone())]);

    let before = mock.count();
    let d = store.generate_review(
        &ReviewItem {
            name: Some(json!("Another Widget")),
            brand: Some(json!("Acme")),
            features: Some(json!("- f")),
            asin: Some(json!("B0STYLEDD4")),
        },
        4.0,
        &[],
        "balanced",
        None,
    );
    assert_eq!((d.title, d.text), (json!("New T"), json!("New body")));
    let (url, body) = mock.calls.lock().unwrap()[before].clone();
    assert!(url.contains("/models/gemini-test-model:generateContent"), "{}", url);
    let body = js::json_parse(&body).unwrap();
    assert_eq!(body["generationConfig"]["temperature"], json!(0.4));
    let prompt = body["contents"][0]["parts"][0]["text"].as_str().unwrap().to_string();
    assert!(prompt.contains("BEGIN MY PAST REVIEWS"));
    assert!(prompt.contains("My posted title") && prompt.contains(&long_body));
    assert!(!prompt.contains("Too short."));
    assert!(prompt.contains("# Variety (important)"));
    assert!(prompt.contains("\"One thing that could be better is\""));

    let mut plain = store_with(t.path(), json!({ "geminiApiKey": "fake-key", "styleExemplars": false }));
    plain.gemini_endpoint = endpoint(&mock.url);
    plain.generate_review(&ReviewItem { name: Some(json!("W")), ..Default::default() }, 5.0, &[], "balanced", None);
    let (url, body) = mock.calls.lock().unwrap().last().unwrap().clone();
    assert!(!body.contains("BEGIN MY PAST REVIEWS"));
    assert!(url.contains("/models/gemini-3.1-flash-lite:generateContent"));
}

// ---------------------------------------------------------------------------
// Headless runs
// ---------------------------------------------------------------------------

#[test]
fn t24_t27_notifications_and_history() {
    let _g = global();
    let t = TempDir::new("history");
    let fake = fake_browser(t.path(), "fake", None);
    let h = Headless::new_for_tests(t.path().join("notes"), true, &fake, true, t.path().join("profile"));
    std::fs::create_dir_all(t.path().join("notes")).unwrap();
    let cursor = logbuf::read_notifications(None)["next"].as_f64().unwrap();
    let run = |processed: i64, ok: bool| {
        let th = start_run(&h);
        let payload = if ok { json!({ "ok": true, "processed": processed }) } else { json!({ "ok": false, "error": "boom-failure" }) };
        assert!(h.complete(&payload));
        done(th.join().unwrap())
    };
    assert_eq!(run(2, true)["lastResult"], "ok");
    assert_eq!(run(0, false)["lastResult"], "error");
    run(0, true);
    let notes_seen: Vec<(String, String)> = logbuf::read_notifications(Some(cursor))["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["title"].as_str().unwrap().into(), n["body"].as_str().unwrap().into()))
        .collect();
    assert_eq!(
        notes_seen,
        vec![("Amazon Order Sync".into(), "2 orders synced".into()), ("Amazon Order Sync failed".into(), "boom-failure".into())],
        "a zero-order run stays silent"
    );
    let st = js::json_parse(&std::fs::read_to_string(t.path().join("notes").join(".headless-sync.json")).unwrap()).unwrap();
    let hist = st["history"].as_array().unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!(hist[2]["ordersSynced"], 2, "newest first");
    assert!(hist.iter().all(|e| e["durationMs"].as_f64().unwrap() >= 0.0));

    // notificationsEnabled false: silent.
    let silent = Headless::new_for_tests(t.path().join("notes2"), true, &fake, false, t.path().join("profile2"));
    std::fs::create_dir_all(t.path().join("notes2")).unwrap();
    let c2 = logbuf::read_notifications(None)["next"].as_f64().unwrap();
    let th = start_run(&silent);
    silent.complete(&json!({ "ok": true, "processed": 5 }));
    th.join().unwrap();
    assert_eq!(logbuf::read_notifications(Some(c2))["items"], json!([]));

    // A state file with 21 entries loads as 20; meta serves 10.
    let capped = t.path().join("capped");
    std::fs::create_dir_all(&capped).unwrap();
    let seeded: Vec<Value> =
        (0..21).map(|i| json!({ "t": 1000 + i, "result": "ok", "error": null, "ordersSynced": 0, "durationMs": 1 })).collect();
    std::fs::write(capped.join(".headless-sync.json"), js::stringify(&json!({ "history": seeded })) + "\n").unwrap();
    let hc = Headless::new_for_tests(capped, false, &fake, false, t.path().join("profile3"));
    hc.load_state();
    assert_eq!(hc.state().history.len(), 20);
    assert_eq!(hc.meta()["history"].as_array().unwrap().len(), 10);
}

#[test]
fn t53_stale_profile_locks_removed() {
    let _g = global();
    let t = TempDir::new("locks");
    let fake = fake_browser(t.path(), "fake", None);
    let profile = t.path().join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    for n in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        std::fs::write(profile.join(n), "stale").unwrap();
    }
    std::fs::create_dir_all(t.path().join("notes")).unwrap();
    let h = Headless::new_for_tests(t.path().join("notes"), true, &fake, false, profile.clone());
    let c = log_cursor();
    let th = start_run(&h);
    for n in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        assert!(std::fs::symlink_metadata(profile.join(n)).is_err(), "{} removed before the spawn", n);
    }
    let lines: Vec<String> = logs_since(c).into_iter().filter(|l| l.contains("Removed stale browser profile lock(s)")).collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("SingletonLock") && lines[0].contains("SingletonSocket") && lines[0].contains("SingletonCookie"));
    assert!(h.complete(&json!({ "ok": true, "processed": 0 })));
    assert_eq!(done(th.join().unwrap())["lastResult"], "ok");
}

#[test]
fn t57_dismiss_during_a_confirming_write_stays_dismissed() {
    let t = TempDir::new("race");
    let h = Headless::new_for_tests(t.path().to_path_buf(), true, "", false, t.path().join("profile"));
    let asin = "B0RACE0001";
    h.record_missing_sighting(asin, "Race Widget");
    assert_eq!(h.state().missing(asin).unwrap().sightings, 1);
    *h.state_save_delay_once.lock().unwrap() = 400;
    let h2 = Arc::clone(&h);
    let confirm = std::thread::spawn(move || h2.record_missing_sighting(asin, "Race Widget"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.clear_missing_review(asin), "the entry exists mid-write, so the dismiss succeeds");
    confirm.join().unwrap();
    assert!(h.state().missing(asin).is_none());
    let on_disk = js::json_parse(&std::fs::read_to_string(t.path().join(".headless-sync.json")).unwrap()).unwrap();
    assert!(on_disk["reviewMissing"].get(asin).is_none(), "the in-flight write must not bring it back");
}

#[test]
fn t61_browser_cache_keys_and_enoent() {
    let _g = global();
    headless::reset_browser_cache_for_tests();
    let t = TempDir::new("cache");
    let fake = fake_browser(t.path(), "fake", None);
    // A real program, so that once it is deleted the spawn itself fails (a
    // .cmd would still start cmd.exe).
    #[cfg(windows)]
    let gone = {
        let p = t.path().join("gone.exe");
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        std::fs::copy(Path::new(&sys).join("System32").join("PING.EXE"), &p).unwrap();
        p.to_string_lossy().into_owned()
    };
    #[cfg(unix)]
    let gone = fake_browser(t.path(), "gone", None);
    let a = Headless::new_for_tests(t.path().join("a"), false, &fake, false, t.path().join("pa"));
    let b = Headless::new_for_tests(t.path().join("b"), false, &gone, false, t.path().join("pb"));
    std::fs::create_dir_all(t.path().join("b")).unwrap();
    assert_eq!(a.resolve_browser(), Some(PathBuf::from(&fake)));
    assert_eq!(b.resolve_browser(), Some(PathBuf::from(&gone)));
    assert_eq!(a.resolve_browser(), Some(PathBuf::from(&fake)), "B never contaminates A");
    std::fs::remove_file(&gone).unwrap();
    let r = done(b.run_headless_sync(None));
    assert_eq!(r["lastResult"], "error");
    assert_eq!(r["lastError"], "browser failed to start");
    assert_ne!(b.resolve_browser(), Some(PathBuf::from(&gone)), "the failed spawn invalidated the cache");
    assert_eq!(a.resolve_browser(), Some(PathBuf::from(&fake)));
    headless::reset_browser_cache_for_tests();
}

#[cfg(unix)]
#[test]
fn t65_browser_cache_ttl_revalidates() {
    use std::os::unix::fs::PermissionsExt;
    let _g = global();
    headless::reset_browser_cache_for_tests();
    headless::set_browser_cache_ttl_for_tests(Some(60));
    let t = TempDir::new("ttl");
    let fake = fake_browser(t.path(), "ttl", None);
    let empty = t.path().join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let old_path = std::env::var_os("PATH");
    std::env::set_var("PATH", &empty);
    let h = Headless::new_for_tests(t.path().join("n"), false, &fake, false, t.path().join("p"));
    let mode = |m: u32| std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(m)).unwrap();
    assert_eq!(h.resolve_browser(), Some(PathBuf::from(&fake)));
    mode(0o644);
    assert_eq!(h.resolve_browser(), Some(PathBuf::from(&fake)), "within the TTL the cached hit is served");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.resolve_browser(), None, "past the TTL it is re-validated");
    mode(0o755);
    assert_eq!(h.resolve_browser(), None, "a cached miss is served within its TTL too");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.resolve_browser(), Some(PathBuf::from(&fake)));
    match old_path {
        Some(p) => std::env::set_var("PATH", p),
        None => std::env::remove_var("PATH"),
    }
    headless::set_browser_cache_ttl_for_tests(None);
    headless::reset_browser_cache_for_tests();
}

#[test]
fn t55_login_block_survives_a_restart() {
    let t = TempDir::new("login");
    std::fs::write(t.path().join(".headless-sync.json"), "{\"lastResult\":\"error\",\"lastError\":\"login-required\"}\n").unwrap();
    let h = Headless::new_for_tests(t.path().to_path_buf(), true, "", false, t.path().join("p"));
    h.load_state();
    assert!(h.login_blocked(), "the gate survives a daemon restart");
    let st = |r: Option<&str>, e: Option<&str>| headless::State {
        last_result: r.map(String::from),
        last_error: e.map(String::from),
        ..Default::default()
    };
    assert!(headless::is_login_blocked(&st(Some("error"), Some("login-required"))));
    assert!(headless::is_login_blocked(&st(Some("error"), Some("Please login again"))));
    assert!(!headless::is_login_blocked(&st(Some("error"), Some("no-browser"))));
    assert!(!headless::is_login_blocked(&st(Some("ok"), None)));
    assert!(!headless::is_login_blocked(&st(None, None)));
}

#[test]
fn t64_instance_lock_acquire_release() {
    let t = TempDir::new("lock");
    crate::daemon::acquire_instance_lock(t.path(), 27483).unwrap();
    let lock = js::json_parse(&std::fs::read_to_string(t.path().join(".daemon.lock")).unwrap()).unwrap();
    assert_eq!(lock["pid"], json!(std::process::id()));
    assert_eq!(lock["port"], json!(27483));
    assert!(lock["startedAt"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
    crate::daemon::release_instance_lock(t.path());
    assert!(!t.path().join(".daemon.lock").exists(), "release removes our lock");
    // A dead pid is taken over; a foreign live one is refused; ours is not removed by others.
    std::fs::write(t.path().join(".daemon.lock"), "{\"pid\": 2147483000}\n").unwrap();
    crate::daemon::acquire_instance_lock(t.path(), 1).unwrap();
    std::fs::write(t.path().join(".daemon.lock"), "not json").unwrap();
    crate::daemon::acquire_instance_lock(t.path(), 1).unwrap();
    std::fs::write(t.path().join(".daemon.lock"), "{\"pid\": 2147483000}\n").unwrap();
    crate::daemon::release_instance_lock(t.path());
    assert!(t.path().join(".daemon.lock").exists(), "someone else's lock is left alone");
}

#[test]
fn t70_config_validation() {
    let cfg = |v: Value| config::from_value(PathBuf::from("c.json"), &v);
    assert_eq!(cfg(json!({})).review_cutoff_days, 60);
    assert_eq!(cfg(json!({ "reviewCutoffDays": -5 })).review_cutoff_days, 60);
    assert_eq!(cfg(json!({ "reviewCutoffDays": 1.5 })).review_cutoff_days, 60);
    assert_eq!(cfg(json!({ "reviewCutoffDays": 0 })).review_cutoff_days, 0);
    assert_eq!(cfg(json!({ "reviewCutoffDays": 30.0 })).review_cutoff_days, 30);
    let c = cfg(
        json!({ "port": 70000, "geminiModel": "bad model!", "geminiTemperature": 3, "syncReminderDays": 0, "notificationsEnabled": "no", "autoSyncEnabled": "yes" }),
    );
    assert_eq!(c.port, 27483);
    assert_eq!(c.gemini_model, config::DEFAULT_GEMINI_MODEL);
    assert_eq!(c.gemini_temperature, config::DEFAULT_GEMINI_TEMPERATURE);
    assert_eq!(c.sync_reminder_days, 7);
    assert!(c.notifications_enabled, "only a literal false disables");
    assert!(!c.auto_sync_enabled, "only a literal true enables");
    let c = cfg(json!({ "geminiModel": " gemini-x.1 ", "geminiTemperature": 0, "port": 8080, "styleExemplars": false }));
    assert_eq!((c.gemini_model.as_str(), c.gemini_temperature, c.port, c.style_exemplars), ("gemini-x.1", 0.0, 8080, false));
}

#[test]
fn t72_stuck_run_recovery() {
    let t = TempDir::new("stuck");
    let h = Headless::new_for_tests(t.path().to_path_buf(), true, "", false, t.path().join("p"));
    // Running with nothing pending: a stop force-recovers instead of 409ing.
    h.force_stuck_for_tests(false);
    assert_eq!(h.stop(), Some(false));
    assert!(!h.running());
    assert_eq!(h.run_started_at_for_tests(), None, "run hygiene fields cleared");
    assert_eq!(h.run_options(), headless::default_run_options());
    assert_eq!(h.stop(), None, "idle: the plain 409");
    // The scheduler's sweep clears the same state with no stop call.
    h.force_stuck_for_tests(false);
    assert!(h.sweep_stuck_run_flag());
    assert!(!h.running());
    assert!(!h.sweep_stuck_run_flag(), "idle is left alone");
    // A run that is really pending is left alone, and stops as cancelled.
    h.force_stuck_for_tests(true);
    assert!(!h.sweep_stuck_run_flag());
    assert!(h.running());
    assert_eq!(h.stop(), Some(true));
    let o = h.take_outcome_for_tests().unwrap();
    assert_eq!((o.last_result.as_str(), o.last_error, o.orders_synced), ("cancelled", None, 0));
}

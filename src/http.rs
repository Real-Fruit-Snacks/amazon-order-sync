//! The HTTP API on 127.0.0.1: the host and origin locks, every /api route,
//! the dashboard and its fonts and icons, and cached product images.

use crate::backfill::{self, CsvFile};
use crate::gemini::{self, ReviewItem};
use crate::headless::{Headless, RunResult};
use crate::js::{self, to_str, to_str_or_empty};
use crate::jsdate::{self, now_ms};
use crate::logbuf;
use crate::notes::{self, guidance_for, prop};
use crate::store::{self, asset_type, js_truthy, Store};
use crate::{elog, log};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const EXTENSION_ID: &str = "cikcnmangaegpkglmcfgbfbmknjmdifh";
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_IMPORT_BODY_BYTES: usize = 50 * 1024 * 1024;
const DAY_MS: f64 = 86_400_000.0;
const REVIEW_CHECK_QUEUE_CAP: usize = 20;

pub const DASHBOARD_HTML: &str = include_str!("../server/dashboard.html");

pub fn font(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "IBMPlexMono-Medium.woff2" => include_bytes!("../server/fonts/IBMPlexMono-Medium.woff2"),
        "IBMPlexMono-Regular.woff2" => include_bytes!("../server/fonts/IBMPlexMono-Regular.woff2"),
        "IBMPlexMono-SemiBold.woff2" => include_bytes!("../server/fonts/IBMPlexMono-SemiBold.woff2"),
        "IBMPlexSans-Bold.woff2" => include_bytes!("../server/fonts/IBMPlexSans-Bold.woff2"),
        "IBMPlexSans-Regular.woff2" => include_bytes!("../server/fonts/IBMPlexSans-Regular.woff2"),
        "IBMPlexSans-SemiBold.woff2" => include_bytes!("../server/fonts/IBMPlexSans-SemiBold.woff2"),
        _ => return None,
    })
}

pub fn icon(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "apple-touch-icon.png" => include_bytes!("../server/icons/apple-touch-icon.png"),
        "favicon.ico" => include_bytes!("../server/icons/favicon.ico"),
        "icon-192.png" => include_bytes!("../server/icons/icon-192.png"),
        "icon-512.png" => include_bytes!("../server/icons/icon-512.png"),
        _ => return None,
    })
}

pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else {
        "linux"
    }
}

pub struct App {
    pub store: Arc<Store>,
    pub headless: Arc<Headless>,
    pub port: u16,
    pub bound_port: std::sync::atomic::AtomicU16,
    pub on_shutdown: Arc<dyn Fn() + Send + Sync>,
}

pub struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    pub fn json(status: u16, v: &Value) -> Resp {
        Resp { status, headers: vec![("Content-Type".into(), "application/json".into())], body: js::stringify(v).into_bytes() }
    }
    fn raw(status: u16, ctype: &str, body: Vec<u8>) -> Resp {
        Resp { status, headers: vec![("Content-Type".into(), ctype.into())], body }
    }
    fn header(mut self, k: &str, v: &str) -> Resp {
        self.headers.push((k.into(), v.into()));
        self
    }
    fn empty(status: u16) -> Resp {
        Resp { status, headers: Vec::new(), body: Vec::new() }
    }
}

fn err(status: u16, msg: &str) -> Resp {
    Resp::json(status, &json!({ "error": msg }))
}

pub fn is_allowed_origin(origin: &str, port: u16) -> bool {
    if origin.is_empty() {
        return true;
    }
    if origin == format!("chrome-extension://{}", EXTENSION_ID) {
        return true;
    }
    let Ok(u) = url::Url::parse(origin) else { return false };
    let Some(host) = u.host_str() else { return false };
    let host = host.to_lowercase();
    if host == "127.0.0.1" || host == "localhost" {
        let p = match u.port() {
            Some(p) => p,
            None => {
                if u.scheme() == "https" {
                    443
                } else {
                    80
                }
            }
        };
        return p == port;
    }
    false
}

pub fn is_allowed_host(host: &str, port: u16) -> bool {
    let h = host.to_lowercase();
    h == format!("127.0.0.1:{}", port) || h == format!("localhost:{}", port)
}

struct Req<'a> {
    method: String,
    url: String,
    host: String,
    origin: Option<String>,
    content_type: String,
    inner: &'a mut tiny_http::Request,
}

enum Body {
    Text(String),
    TooLarge,
}

impl Req<'_> {
    fn body(&mut self, limit: usize) -> Body {
        let mut buf = Vec::new();
        let mut r = self.inner.as_reader().take(limit as u64 + 1);
        if r.read_to_end(&mut buf).is_err() {
            return Body::Text(String::from_utf8_lossy(&buf).into_owned());
        }
        if buf.len() > limit {
            return Body::TooLarge;
        }
        Body::Text(String::from_utf8_lossy(&buf).into_owned())
    }

    fn parsed_url(&self) -> Option<url::Url> {
        let base = url::Url::parse(&format!("http://{}", self.host)).ok()?;
        base.join(&self.url).ok()
    }

    fn query(&self, name: &str) -> Option<String> {
        self.parsed_url()?.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
    }
}

/// readJsonBody: 413 past the cap, 400 on malformed JSON, else the handler;
/// a handler error is a 500 with its message.
fn with_json(req: &mut Req, limit: usize, f: impl FnOnce(Value) -> Result<Resp, String>) -> Resp {
    match req.body(limit) {
        Body::TooLarge => err(413, "Payload too large"),
        Body::Text(t) => match js::json_parse(&t) {
            None => err(400, "Invalid JSON body"),
            Some(v) => match f(v) {
                Ok(r) => r,
                Err(e) => {
                    elog!("Error handling request: {}: {}", ename(&e), e);
                    err(500, &e)
                }
            },
        },
    }
}

fn null_prop_err(payload: &Value, key: &str) -> Result<(), String> {
    if payload.is_null() {
        return Err(format!("Cannot read properties of null (reading '{}')", key));
    }
    Ok(())
}

fn cursor(req: &Req) -> Option<f64> {
    let base = url::Url::parse("http://127.0.0.1").ok()?;
    let u = base.join(&req.url).ok()?;
    let raw = u.query_pairs().find(|(k, _)| k == "after").map(|(_, v)| v.into_owned())?;
    if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
        raw.parse::<f64>().ok()
    } else {
        None
    }
}

fn header_value(r: &tiny_http::Request, name: &'static str, join: bool) -> Option<String> {
    let vals: Vec<String> = r.headers().iter().filter(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string()).collect();
    if vals.is_empty() {
        None
    } else if join {
        Some(vals.join(", "))
    } else {
        Some(vals[0].clone())
    }
}

pub fn handle(app: &App, mut request: tiny_http::Request) {
    let bound = app.bound_port.load(std::sync::atomic::Ordering::SeqCst);
    let bound = if bound == 0 { app.port } else { bound };
    let method = request.method().as_str().to_uppercase();
    let url = request.url().to_string();
    let host = header_value(&request, "Host", false).unwrap_or_default();
    let origin = header_value(&request, "Origin", true);
    let content_type = header_value(&request, "Content-Type", false).unwrap_or_default();

    let resp = if !is_allowed_host(&host, bound) {
        err(403, "Forbidden host")
    } else {
        let origin_s = origin.clone().unwrap_or_default();
        let allowed = is_allowed_origin(&origin_s, bound);
        let mut cors: Vec<(String, String)> = Vec::new();
        if allowed && !origin_s.is_empty() {
            cors.push(("Access-Control-Allow-Origin".into(), origin_s.clone()));
            cors.push(("Vary".into(), "Origin".into()));
        }
        cors.push(("Access-Control-Allow-Methods".into(), "GET, POST, OPTIONS".into()));
        cors.push(("Access-Control-Allow-Headers".into(), "Content-Type".into()));
        let mut r = if method == "OPTIONS" {
            Resp::empty(if allowed { 204 } else { 403 })
        } else if !allowed {
            err(403, "Forbidden origin")
        } else {
            let mut req = Req { method, url, host, origin, content_type, inner: &mut request };
            route(app, &mut req)
        };
        let mut h = cors;
        h.append(&mut r.headers);
        r.headers = h;
        r
    };

    let mut out = tiny_http::Response::from_data(resp.body).with_status_code(resp.status);
    for (k, v) in resp.headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            out.add_header(h);
        }
    }
    let _ = request.respond(out);
}

fn route(app: &App, req: &mut Req) -> Resp {
    let store = &app.store;
    let headless = &app.headless;
    let _ = &req.origin;
    let pathname = req.parsed_url().map(|u| u.path().to_string()).unwrap_or_else(|| req.url.clone());
    let m = req.method.as_str();
    let p = pathname.as_str();

    match (m, p) {
        ("GET", "/api/check") => {
            let r = (|| -> Result<Resp, String> {
                let Some(order_id) = req.query("order_id").filter(|s| !s.is_empty()) else {
                    return Ok(err(400, "Missing order_id"));
                };
                let safe = notes::sanitize_id_str(&order_id);
                if !notes::is_safe_id(&safe) {
                    return Ok(err(400, "Invalid order_id"));
                }
                Ok(analyze_order(store, &safe))
            })();
            r.unwrap_or_else(|e| {
                elog!("Error checking order: {}", e);
                err(500, &e)
            })
        }
        ("GET", "/api/get-review") => {
            let Some(asin) = req.query("asin").filter(|s| !s.is_empty()) else { return err(400, "Missing asin") };
            let safe = notes::sanitize_id_str(&asin);
            if !notes::is_safe_id(&safe) {
                return err(400, "Invalid asin");
            }
            find_review(store, &safe)
        }
        ("POST", "/api/sync") => match req.body(MAX_BODY_BYTES) {
            Body::TooLarge => err(413, "Payload too large"),
            Body::Text(t) => {
                let Some(mut payload) = js::json_parse(&t) else { return err(400, "Invalid JSON body") };
                match sync_order(app, &mut payload) {
                    Ok(()) => Resp::json(200, &json!({ "success": true })),
                    Err(e) => {
                        elog!("Error processing payload: {}: {}", ename(&e), e);
                        err(500, &e)
                    }
                }
            }
        },
        ("GET", "/api/health") => Resp::json(
            200,
            &json!({
                "ok": true,
                "version": VERSION,
                "pid": std::process::id(),
                "platform": platform(),
                "notesPath": store.notes_path.to_string_lossy(),
                "port": app.port,
                "browserFound": headless.resolve_browser().is_some(),
                "headlessRunning": headless.running(),
            }),
        ),
        ("GET", "/api/notifications") => Resp::json(200, &logbuf::read_notifications(cursor(req))),
        ("POST", "/api/shutdown") => {
            let r = Resp::json(200, &json!({ "ok": true }));
            // After the response flushes.
            let f = Arc::clone(&app.on_shutdown);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                f();
            });
            r
        }
        ("GET", "/api/gemini-models") => {
            if store.gemini_api_key.is_empty() {
                return err(400, "No Gemini API key configured");
            }
            let (status, body) = gemini::list_models(&store.gemini_api_key, &store.gemini_model);
            Resp::json(status, &body)
        }
        ("GET", "/api/gemini-usage") => {
            let u = gemini::load_usage(&store.notes_path);
            Resp::json(
                200,
                &json!({
                    "used": u.used,
                    "limit": store.gemini_daily_limit,
                    "remaining": (store.gemini_daily_limit - u.used).max(0),
                    "day": u.day,
                    "configured": !store.gemini_api_key.is_empty(),
                }),
            )
        }
        ("GET", "/api/run-options") => Resp::json(200, &json!({ "options": headless.run_options() })),
        ("GET", "/api/logs") => Resp::json(200, &logbuf::read_logs(cursor(req))),
        ("POST", "/api/logs/clear") => {
            logbuf::clear_logs();
            Resp::json(200, &json!({ "ok": true }))
        }
        ("POST", "/api/sync-history/clear") => {
            headless.clear_history();
            Resp::json(200, &json!({ "ok": true }))
        }
        ("GET", "/api/orders") => orders_route(app),
        ("GET", _) if req.url.starts_with("/assets/") => serve_asset(store, &pathname),
        ("GET", _) if req.url.starts_with("/fonts/") => serve_embedded(&pathname, "/fonts/", |n, ext| {
            if ext != "woff2" {
                return None;
            }
            font(n).map(|b| ("font/woff2", b))
        }),
        ("GET", _) if p == "/favicon.ico" || p.starts_with("/icons/") => {
            let prefix = if p == "/favicon.ico" { "" } else { "/icons/" };
            serve_embedded(if p == "/favicon.ico" { "favicon.ico" } else { p }, prefix, |n, ext| {
                if ext != "png" && ext != "ico" {
                    return None;
                }
                icon(n).map(|b| (if ext == "ico" { "image/x-icon" } else { "image/png" }, b))
            })
        }
        ("POST", "/api/mark-posted") => match req.body(MAX_BODY_BYTES) {
            Body::TooLarge => err(413, "Payload too large"),
            Body::Text(t) => {
                let Some(payload) = js::json_parse(&t) else { return err(400, "Invalid JSON body") };
                let r = (|| -> Result<Resp, String> {
                    null_prop_err(&payload, "asin")?;
                    let safe = notes::sanitize_id(prop(&payload, "asin"));
                    if !notes::is_safe_id(&safe) {
                        return Ok(err(400, "Invalid asin"));
                    }
                    let posted = prop(&payload, "posted") == Some(&Value::Bool(true));
                    let Some((file, content)) = store.find_latest_note_with_asin(&safe) else {
                        return Ok(err(404, "ASIN not found in any order note"));
                    };
                    let (updated, changed) = notes::set_checkbox(&content, &safe, posted);
                    if changed {
                        store.write_markdown_atomic(&file, &updated).map_err(|e| e.to_string())?;
                    }
                    store.invalidate_scan_cache();
                    Ok(Resp::json(200, &json!({ "success": true, "posted": posted })))
                })();
                r.unwrap_or_else(|e| {
                    elog!("Error marking item posted: {}: {}", ename(&e), e);
                    err(500, &e)
                })
            }
        },
        ("POST", "/api/save-review") => with_json(req, MAX_BODY_BYTES, |payload| {
            null_prop_err(&payload, "asin")?;
            let safe = notes::sanitize_id(prop(&payload, "asin"));
            if !notes::is_safe_id(&safe) {
                return Ok(err(400, "Invalid asin"));
            }
            let (Some(Value::String(title)), Some(Value::String(text))) = (prop(&payload, "title"), prop(&payload, "text")) else {
                return Ok(err(400, "Invalid payload: title and text must be strings and text must be non-empty"));
            };
            if js::trim(text).is_empty() {
                return Ok(err(400, "Invalid payload: title and text must be strings and text must be non-empty"));
            }
            let rating = match optional_rating(&payload) {
                Ok(r) => r,
                Err(resp) => return Ok(resp),
            };
            let Some((file, content)) = store.find_latest_note_with_asin(&safe) else {
                return Ok(err(404, "ASIN not found in any order note"));
            };
            let Some(updated) = notes::update_review_block(&content, &safe, title, text, rating) else {
                return Ok(err(404, "Review block not found for ASIN"));
            };
            store.write_markdown_atomic(&file, &updated).map_err(|e| e.to_string())?;
            store.invalidate_scan_cache();
            let name = notes::find_item_block(&updated, &safe)
                .and_then(|loc| {
                    let block = &updated[loc.start..loc.end];
                    js::find(js::re(r"^###\s+(.+)$", "m"), block).map(|m| js::trim(m.group(block, 1).unwrap_or("")).to_string())
                })
                .unwrap_or_default();
            Ok(Resp::json(200, &json!({ "ok": true, "item_name": name })))
        }),
        ("POST", "/api/regenerate-review") => with_json(req, MAX_BODY_BYTES, |payload| regenerate(store, &payload)),
        ("GET", "/api/export.csv") => {
            let orders = store.scan_notes();
            let mut rows = vec!["order_id,order_date,item_name,brand,asin,price,url,rating,reviewed,posted,helpful_votes".to_string()];
            for o in &orders {
                for it in o["items"].as_array().into_iter().flatten() {
                    let hv = match it.get("helpfulVotes") {
                        None | Some(Value::Null) => json!(0),
                        Some(v) => v.clone(),
                    };
                    let fields = [
                        o.get("orderId"),
                        o.get("date"),
                        it.get("name"),
                        it.get("brand"),
                        it.get("asin"),
                        it.get("price"),
                        it.get("url"),
                        it.get("rating"),
                        it.get("reviewed"),
                        it.get("posted"),
                        Some(&hv),
                    ];
                    rows.push(fields.iter().map(|f| notes::csv_escape(&to_str_or_empty(*f))).collect::<Vec<_>>().join(","));
                }
            }
            let date = &jsdate::iso_string(now_ms())[..10];
            Resp::raw(200, "text/csv; charset=utf-8", (rows.join("\n") + "\n").into_bytes())
                .header("Content-Disposition", &format!("attachment; filename=\"amazon-orders-{}.csv\"", date))
        }
        ("GET", "/api/export.json") => {
            let orders = store.scan_notes();
            let date = jsdate::iso_string(now_ms());
            let mut o = Map::new();
            o.insert("exported_at".into(), json!(jsdate::iso_string(now_ms())));
            o.insert("version".into(), json!(VERSION));
            o.insert("orders".into(), Value::Array(orders));
            Resp::raw(200, "application/json; charset=utf-8", (js::stringify_pretty(&Value::Object(o)) + "\n").into_bytes())
                .header("Content-Disposition", &format!("attachment; filename=\"amazon-orders-{}.json\"", &date[..10]))
        }
        ("POST", "/api/import-backfill") => {
            if req.content_type.to_lowercase().starts_with("text/csv") {
                match req.body(MAX_IMPORT_BODY_BYTES) {
                    Body::TooLarge => err(413, "Payload too large"),
                    Body::Text(t) => {
                        let result = backfill::import_files(store, &[CsvFile { name: "upload.csv".into(), content: t }]);
                        store.invalidate_scan_cache();
                        Resp::json(200, &result)
                    }
                }
            } else {
                with_json(req, MAX_IMPORT_BODY_BYTES, |payload| {
                    let bad = || err(400, "Invalid body: expected {files:[{name, content}]} or text/csv");
                    let Some(Value::Array(files)) = prop(&payload, "files") else { return Ok(bad()) };
                    if files.iter().any(|f| !f.is_object() || !matches!(f.get("content"), Some(Value::String(_)))) {
                        return Ok(bad());
                    }
                    let files: Vec<CsvFile> = files
                        .iter()
                        .enumerate()
                        .map(|(i, f)| CsvFile {
                            name: match f.get("name") {
                                Some(Value::String(n)) if !n.is_empty() => n.clone(),
                                _ => format!("file-{}.csv", i + 1),
                            },
                            content: f["content"].as_str().unwrap_or("").to_string(),
                        })
                        .collect();
                    let result = backfill::import_files(store, &files);
                    store.invalidate_scan_cache();
                    Ok(Resp::json(200, &result))
                })
            }
        }
        ("GET", "/api/review-check-queue") => {
            let asins = compute_review_check_queue(store, headless, store.review_check_interval_days, now_ms() as f64);
            Resp::json(200, &json!({ "asins": asins }))
        }
        ("POST", "/api/review-check-result") => with_json(req, MAX_BODY_BYTES, |payload| review_check_result(app, &payload)),
        ("POST", "/api/dismiss-missing") => with_json(req, MAX_BODY_BYTES, |payload| {
            null_prop_err(&payload, "asin")?;
            let safe = notes::sanitize_id(prop(&payload, "asin"));
            if !notes::is_safe_id(&safe) {
                return Ok(err(400, "Invalid asin"));
            }
            if !headless.clear_missing_review(&safe) {
                return Ok(err(404, "No missing-review entry for ASIN"));
            }
            Ok(Resp::json(200, &json!({ "ok": true })))
        }),
        ("POST", "/api/review-check-now") => {
            if headless.login_blocked() {
                return Resp::json(
                    409,
                    &json!({ "error": "login-required — run SETUP LOGIN first", "guidance": guidance_for("login-required") }),
                );
            }
            if headless.setup_browser_open() {
                return err(409, "setup browser open — close it first");
            }
            if headless.running() {
                return err(409, "sync already in progress");
            }
            let asins = compute_review_check_queue(store, headless, store.review_check_interval_days, now_ms() as f64);
            if asins.is_empty() {
                return Resp::json(200, &json!({ "ok": true, "result": { "lastResult": "ok", "ordersSynced": 0 } }));
            }
            match headless.run_review_check(&asins[0]) {
                RunResult::Already => err(409, "sync already in progress"),
                RunResult::NoBrowser(r) | RunResult::Done(r) => Resp::json(200, &json!({ "ok": true, "result": r })),
            }
        }
        ("POST", "/api/headless-complete") => with_json(req, MAX_BODY_BYTES, |payload| {
            if !headless.complete(&payload) {
                return Ok(err(409, "no run in progress"));
            }
            Ok(Resp::json(200, &json!({ "ok": true })))
        }),
        ("POST", "/api/client-log") => with_json(req, MAX_BODY_BYTES, |payload| {
            let lines = match prop(&payload, "lines") {
                Some(Value::Array(a)) if !a.is_empty() && a.len() <= 100 && a.iter().all(|l| l.is_string()) => a.clone(),
                _ => return Ok(err(400, "lines must be an array of 1..100 strings")),
            };
            for l in &lines {
                logbuf::push_log_line(format!("[ext] {}", js::utf16_prefix(l.as_str().unwrap_or(""), 500)));
            }
            headless.note_client_log_lines(lines.len() as i64);
            Ok(Resp::json(200, &json!({ "ok": true, "accepted": lines.len() })))
        }),
        ("POST", "/api/sync-now") => {
            if headless.login_blocked() {
                return Resp::json(
                    409,
                    &json!({ "error": "login-required — run SETUP LOGIN first", "guidance": guidance_for("login-required") }),
                );
            }
            if headless.setup_browser_open() {
                return err(409, "setup browser open — close it first");
            }
            if headless.running() {
                return err(409, "sync already in progress");
            }
            let body = match req.body(MAX_BODY_BYTES) {
                Body::TooLarge => return err(413, "Payload too large"),
                Body::Text(t) => t,
            };
            if headless.setup_browser_open() {
                return err(409, "setup browser open — close it first");
            }
            if headless.running() {
                return err(409, "sync already in progress");
            }
            let payload = if js::trim(&body).is_empty() { None } else { js::json_parse(&body) };
            match headless.run_headless_sync(payload.as_ref()) {
                RunResult::Already => err(409, "sync already in progress"),
                RunResult::NoBrowser(r) | RunResult::Done(r) => Resp::json(200, &json!({ "ok": true, "result": r })),
            }
        }
        ("POST", "/api/sync-stop") => match headless.stop() {
            None => err(409, "no run in progress"),
            Some(false) => Resp::json(200, &json!({ "ok": true, "recovered": true })),
            Some(true) => Resp::json(200, &json!({ "ok": true })),
        },
        ("POST", "/api/headless-setup") => match headless.launch_setup_browser() {
            Ok(()) => Resp::json(200, &json!({ "ok": true })),
            Err(e) if e == "sync already in progress" || e == "no-browser" => err(409, &e),
            Err(e) => {
                elog!("Error launching setup browser: {}: {}", ename(&e), e);
                err(500, &e)
            }
        },
        ("GET", "/") | ("GET", "/dashboard") => Resp::raw(200, "text/html; charset=utf-8", DASHBOARD_HTML.as_bytes().to_vec()),
        _ => err(404, "Not found"),
    }
}

fn optional_rating(payload: &Value) -> Result<Option<f64>, Resp> {
    match prop(payload, "rating") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let r = js::to_number(Some(v));
            if !js::is_integer(r) || !(1.0..=5.0).contains(&r) {
                return Err(err(400, "Invalid rating: must be an integer between 1 and 5"));
            }
            Ok(Some(r))
        }
    }
}

/// Basename-only names with a whitelisted extension, else 404.
fn checked_name(pathname: &str, prefix: &str) -> Option<(String, String)> {
    let raw = if prefix.is_empty() { pathname.to_string() } else { pathname.get(prefix.len()..).unwrap_or("").to_string() };
    let filename = js::decode_uri_component(&raw)?;
    let ext = match filename.rfind('.') {
        Some(d) if d > 0 => filename[d + 1..].to_lowercase(),
        _ => String::new(),
    };
    let drive = cfg!(windows) && filename.len() >= 2 && filename.as_bytes()[1] == b':' && filename.as_bytes()[0].is_ascii_alphabetic();
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename.contains("..") || drive {
        return None;
    }
    Some((filename, ext))
}

fn serve_asset(store: &Store, pathname: &str) -> Resp {
    let Some((name, ext)) = checked_name(pathname, "/assets/") else { return err(404, "Not found") };
    let Some(ctype) = asset_type(&ext) else { return err(404, "Not found") };
    match std::fs::read(store.notes_path.join("Orders").join("assets").join(&name)) {
        Ok(data) => Resp::raw(200, ctype, data).header("Cache-Control", "public, max-age=86400"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => err(404, "Not found"),
        Err(e) => {
            elog!("Error serving asset: Error: {}", e);
            err(500, &e.to_string())
        }
    }
}

fn serve_embedded(pathname: &str, prefix: &str, f: impl Fn(&str, &str) -> Option<(&'static str, &'static [u8])>) -> Resp {
    let Some((name, ext)) = checked_name(pathname, prefix) else { return err(404, "Not found") };
    match f(&name, &ext) {
        Some((ctype, data)) => Resp::raw(200, ctype, data.to_vec()).header("Cache-Control", "public, max-age=604800, immutable"),
        None => err(404, "Not found"),
    }
}

// ---------------------------------------------------------------------------
// Routes with more logic
// ---------------------------------------------------------------------------

fn sync_order(app: &App, payload: &mut Value) -> Result<(), String> {
    if payload.is_null() {
        return Err("Cannot read properties of null (reading 'items')".into());
    }
    if let Some(Value::Array(items)) = prop(payload, "items") {
        let deduped = notes::dedupe_items_by_asin(items, prop(payload, "order_id"));
        payload["items"] = Value::Array(deduped);
    }
    app.store.process_payload(payload)?;
    app.store.invalidate_scan_cache();
    if let Some(Value::Array(items)) = prop(payload, "items") {
        let voted: Vec<String> = items
            .iter()
            .filter(|i| !i.is_null() && notes::non_negative_int(prop(i, "helpful_votes")).is_some())
            .map(|i| to_str(prop(i, "asin")))
            .collect();
        app.headless.bump_vote_timestamps(&voted);
    }
    app.headless.note_order_posted();
    Ok(())
}

fn analyze_order(store: &Store, order_id: &str) -> Resp {
    let Some(file) = store.find_order_file(order_id) else {
        return Resp::json(200, &json!({ "complete": false, "exists": false, "reason": "File does not exist" }));
    };
    let content = match store::read_text(&file) {
        Ok(c) => c,
        Err(e) => return err(500, &e.to_string()),
    };
    let mut existing = Map::new();
    let asins = notes::parse_array_field(&content, "asin");
    let brands = notes::parse_aligned_array(&content, "brand");
    let prices = notes::parse_aligned_array(&content, "price");
    let blocks = js::split(js::re(r"\r?\n---\r?\n", ""), &content);
    for (i, asin) in asins.iter().enumerate() {
        let brand = brands.get(i).filter(|b| !b.is_empty()).cloned().unwrap_or_else(|| "Unknown".into());
        let price = prices.get(i).filter(|b| !b.is_empty()).cloned().unwrap_or_else(|| "Unknown".into());
        let mut features = String::new();
        let mut posted = false;
        if let Some(block) = blocks.iter().find(|b| b.contains(asin.as_str()) && b.contains(notes::REVIEW_HEADER)) {
            let fi = block.find("#### Features");
            let ri = block.find(notes::REVIEW_HEADER);
            if let (Some(fi), Some(ri)) = (fi, ri) {
                if ri > fi {
                    features = js::trim(&block[fi + "#### Features".len()..ri]).to_string();
                }
            }
            posted = notes::is_posted_block(block);
        }
        existing.insert(asin.clone(), json!({ "brand": brand, "current_price": price, "features": features, "alreadyPosted": posted }));
    }
    let needs_review = content.contains("> *Write your review for this item below...*");
    let status_raw = notes::scalar_field(&content, "status");
    let mut undelivered = false;
    if !status_raw.is_empty() {
        let s = status_raw.to_lowercase();
        undelivered = !s.contains("delivered") && !s.contains("return") && !s.contains("cancel");
    }
    let not_posted = content.contains("- [ ] Posted to Amazon?");
    if needs_review || undelivered || not_posted {
        return Resp::json(
            200,
            &json!({ "complete": false, "exists": true, "reason": "Needs update or undelivered", "existingItems": existing }),
        );
    }
    Resp::json(200, &json!({ "complete": true, "exists": true, "existingItems": existing }))
}

fn find_review(store: &Store, asin: &str) -> Resp {
    for file in store.walk_orders() {
        if !file.to_string_lossy().ends_with(".md") {
            continue;
        }
        let Ok(content) = store::read_text(&file) else {
            return err(500, "read failed");
        };
        let Some(fm) = js::find(js::re(r"---[\s\S]*?---", ""), &content) else { continue };
        let fm = fm.text(&content);
        if !fm.contains(&format!("\"{}\"", asin)) {
            continue;
        }
        let Some(am) = js::find(js::re(r"asin:\s*\[(.*?)\]", ""), fm) else { continue };
        let list: Vec<String> = am.group(fm, 1).unwrap_or("").split(',').map(|s| js::trim(s).replace('"', "")).collect();
        let Some(index) = list.iter().position(|a| a == asin) else { continue };
        let blocks: Vec<&str> = content.split(notes::REVIEW_HEADER).collect();
        if blocks.len() <= index + 1 {
            continue;
        }
        let block = blocks[index + 1];
        let t = js::find(js::re(r"\*\*Title:\*\*\s*(.+)", ""), block);
        let r = js::find(js::re(r"\*\*Review:\*\*\s*([\s\S]*?)(?=\n- \[|\n---|$)", ""), block);
        if let (Some(t), Some(r)) = (t, r) {
            let title = js::trim(t.group(block, 1).unwrap_or("")).to_string();
            let text = js::trim(r.group(block, 1).unwrap_or("")).to_string();
            if !title.is_empty() && !text.is_empty() && !text.contains("Write your review for this item below") {
                let rating = js::find(js::re(r"\*\*Rating:\*\*[ \t]*([1-5])(?!\d)", ""), block)
                    .map(|m| json!(m.group(block, 1).unwrap_or("0").parse::<i64>().unwrap_or(0)))
                    .unwrap_or(json!(""));
                return Resp::json(200, &json!({ "title": title, "text": text, "rating": rating }));
            }
        }
    }
    err(404, "Review not found or not generated yet")
}

fn regenerate(store: &Store, payload: &Value) -> Result<Resp, String> {
    null_prop_err(payload, "asin")?;
    let safe = notes::sanitize_id(prop(payload, "asin"));
    if !notes::is_safe_id(&safe) {
        return Ok(err(400, "Invalid asin"));
    }
    let mut rating = match optional_rating(payload) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    let tone = match prop(payload, "tone") {
        None | Some(Value::Null) => "balanced".to_string(),
        Some(v) => to_str(Some(v)),
    };
    if !gemini::VALID_TONES.contains(&tone.as_str()) {
        return Ok(err(400, &format!("Invalid tone: must be one of {}", gemini::VALID_TONES.join(", "))));
    }
    let Some((file, content)) = store.find_latest_note_with_asin(&safe) else {
        return Ok(err(404, "ASIN not found in any order note"));
    };
    let Some(loc) = notes::find_item_block(&content, &safe) else {
        return Ok(err(404, "Review block not found for ASIN"));
    };
    let block = &content[loc.start..loc.end];
    let name = js::find(js::re(r"^###\s+(.+)$", "m"), block)
        .map(|m| js::trim(m.group(block, 1).unwrap_or("")).to_string())
        .unwrap_or_else(|| safe.clone());
    let mut features = String::new();
    if let (Some(fi), Some(ri)) = (block.find("#### Features"), block.find(notes::REVIEW_HEADER)) {
        if ri > fi {
            features = js::trim(&block[fi + "#### Features".len()..ri]).to_string();
        }
    }
    let brand = notes::frontmatter(&content)
        .map(|fm| notes::parse_aligned_array(&fm, "brand").get(loc.index).cloned().unwrap_or_default())
        .unwrap_or_default();
    let rating_v = match rating {
        Some(r) => r,
        None => {
            let r = js::find(js::re(r"\*\*Rating:\*\*\s*([1-5])(?!\d)", ""), block)
                .map(|m| m.group(block, 1).unwrap_or("5").parse::<f64>().unwrap_or(5.0))
                .unwrap_or_else(|| notes::default_rating_for_asin(&safe) as f64);
            rating = Some(r);
            r
        }
    };
    let context = store.load_review_context();
    let top: Vec<String> = context
        .get(&safe)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut title = json!("");
    let mut text = json!("");
    let mut fallback = true;
    if !store.gemini_api_key.is_empty() {
        let before = gemini::load_usage(&store.notes_path);
        if before.used >= store.gemini_daily_limit {
            let (t, x) = notes::fallback_review(rating_v);
            title = json!(t);
            text = json!(x);
        } else {
            let mut cfg = store.gemini_config();
            if let Some(Value::String(m)) = prop(payload, "model") {
                if crate::config::valid_model(js::trim(m)) {
                    cfg.model = js::trim(m).to_string();
                }
            }
            let mut rnd = [0u8; 8];
            let _ = getrandom::fill(&mut rnd);
            let r = u64::from_le_bytes(rnd) as f64 / u64::MAX as f64;
            let seed = notes::seed_from_string(&format!("{}:{}:{}", safe, now_ms(), js::num_to_string(r)));
            let item =
                ReviewItem { name: Some(json!(name)), brand: Some(json!(brand)), features: Some(json!(features)), asin: Some(json!(safe)) };
            let draft = gemini::generate_review(&cfg, &item, rating_v, &top, &tone, Some(seed), &|| store.style_exemplars_for());
            title = draft.title.clone();
            text = draft.text.clone();
            if !draft.fallback && js_truthy(Some(&draft.text)) {
                fallback = false;
            }
        }
    }
    if fallback {
        let (t, x) = notes::fallback_review(rating_v);
        title = json!(t);
        text = json!(x);
    }
    let Some(updated) = notes::update_review_block(&content, &safe, &to_str_or_empty(Some(&title)), &to_str_or_empty(Some(&text)), rating)
    else {
        return Ok(err(500, "Failed to update review block"));
    };
    let with_tone = notes::set_frontmatter_array_value(&updated, "tone", &safe, &tone).unwrap_or(updated);
    store.write_markdown_atomic(&file, &with_tone).map_err(|e| e.to_string())?;
    store.invalidate_scan_cache();
    let model = match prop(payload, "model") {
        Some(Value::String(m)) if !js::trim(m).is_empty() => js::trim(m).to_string(),
        _ => store.gemini_model.clone(),
    };
    Ok(Resp::json(
        200,
        &json!({ "ok": true, "title": title, "text": text, "rating": js::jnum(rating_v), "tone": tone, "fallback": fallback, "model": model }),
    ))
}

fn review_check_result(app: &App, payload: &Value) -> Result<Resp, String> {
    let store = &app.store;
    let headless = &app.headless;
    null_prop_err(payload, "asin")?;
    let safe = notes::sanitize_id(prop(payload, "asin"));
    if !notes::is_safe_id(&safe) {
        return Ok(err(400, "Invalid asin"));
    }
    if let Some(v) = prop(payload, "reviewed") {
        if !v.is_boolean() {
            return Ok(err(400, "Invalid reviewed: must be a boolean"));
        }
    }
    let page_state = match prop(payload, "page_state") {
        None => "loaded".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => String::from("\u{0}"),
    };
    if page_state != "loaded" && page_state != "captcha" && page_state != "error" {
        return Ok(err(400, "Invalid page_state: must be 'loaded', 'captcha' or 'error'"));
    }
    if page_state != "loaded" {
        return Ok(Resp::json(200, &json!({ "ok": true })));
    }
    let reviewed = prop(payload, "reviewed") == Some(&Value::Bool(true));
    let title = prop(payload, "title").and_then(|v| v.as_str());
    let text = prop(payload, "text").and_then(|v| v.as_str());
    if reviewed && (title.map(|t| js::trim(t).is_empty()).unwrap_or(true) || text.map(|t| js::trim(t).is_empty()).unwrap_or(true)) {
        return Ok(err(400, "Invalid payload: reviewed items need non-empty title and text"));
    }
    let mut votes: Option<f64> = None;
    match prop(payload, "helpful_votes") {
        None | Some(Value::Null) => {}
        Some(v) => {
            votes = notes::non_negative_int(Some(v));
            if votes.is_none() {
                return Ok(err(400, "Invalid helpful_votes: must be a non-negative integer"));
            }
        }
    }
    let rating = match optional_rating(payload) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    let Some((file, original)) = store.find_latest_note_with_asin(&safe) else {
        return Ok(err(404, "ASIN not found in any order note"));
    };
    let mut content = original.clone();
    if reviewed {
        let Some(updated) = notes::update_review_block(&original, &safe, title.unwrap_or(""), text.unwrap_or(""), rating) else {
            return Ok(err(404, "Review block not found for ASIN"));
        };
        content = notes::set_checkbox(&updated, &safe, true).0;
    }
    if let Some(v) = votes {
        content = notes::set_frontmatter_array_value(&content, "helpful_votes", &safe, &js::num_to_string(v)).unwrap_or(content);
    }
    if content != original {
        store.write_markdown_atomic(&file, &content).map_err(|e| e.to_string())?;
        store.invalidate_scan_cache();
    }
    if reviewed {
        headless.clear_missing_review(&safe);
    } else if let Some(parsed) = notes::parse_note(&content, "", 0.0, &|_| None) {
        let item = parsed.order["items"].as_array().and_then(|a| a.iter().find(|i| i["asin"] == json!(safe))).cloned();
        if let Some(item) = item {
            if item["posted"] == json!(true) && item["hasDraft"] == json!(true) {
                headless.record_missing_sighting(&safe, &to_str(item.get("name")));
            }
        }
    }
    headless.bump_vote_timestamps(std::slice::from_ref(&safe));
    Ok(Resp::json(200, &json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// /api/orders, the review-check queue, vote-timestamp pruning
// ---------------------------------------------------------------------------

// The negated comparisons are deliberate: NaN (an unparseable price) fails both, as in JS.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn orders_route(app: &App) -> Resp {
    let store = &app.store;
    let headless = &app.headless;
    let price_history = store.load_price_history();
    let missing_state = headless.state().review_missing;
    let cutoff_days = store.review_cutoff_days;
    let mut last_sync_at: Option<f64> = None;
    let mut price_drops: Vec<Value> = Vec::new();
    let mut missing_reviews: Vec<Value> = Vec::new();
    let mut generated_at = 0i64;
    let orders = store.scan_notes_mut(|orders| {
        for o in orders.iter() {
            let m = js::to_number(o.get("modified"));
            let m = if m.is_nan() { 0.0 } else { m };
            if m > 0.0 && last_sync_at.map(|l| m > l).unwrap_or(true) {
                last_sync_at = Some(m);
            }
        }
        let mut counts: HashMap<String, i64> = HashMap::new();
        for o in orders.iter() {
            let mut in_order = HashSet::new();
            for it in o["items"].as_array().into_iter().flatten() {
                let a = asin_of(it);
                if !a.is_empty() {
                    in_order.insert(a);
                }
            }
            for a in in_order {
                *counts.entry(a).or_insert(0) += 1;
            }
        }
        let mut name_by_asin: HashMap<String, Value> = HashMap::new();
        for o in orders.iter_mut() {
            for it in o["items"].as_array_mut().into_iter().flatten() {
                let a = asin_of(it);
                if a.is_empty() {
                    continue;
                }
                let latest = price_history
                    .get(&a)
                    .and_then(|e| e["history"].as_array())
                    .and_then(|h| h.last())
                    .map(|l| l["price"].clone())
                    .unwrap_or(Value::Null);
                let obj = it.as_object_mut().unwrap();
                obj.insert("repeatCount".into(), json!(counts.get(&a).copied().unwrap_or(1)));
                obj.insert("listingPrice".into(), latest);
                name_by_asin.entry(a).or_insert_with(|| obj.get("name").cloned().unwrap_or(Value::Null));
            }
        }
        for (asin, e) in &price_history {
            let Some(last) = e["history"].as_array().and_then(|h| h.last()) else { continue };
            let listing = last["price"].as_f64().unwrap_or(f64::NAN);
            let paid = e["paid"].as_f64().unwrap_or(f64::NAN);
            if !(paid > 0.0) || !(listing < paid) {
                continue;
            }
            let savings = js::string_to_number(&js::to_fixed(paid - listing, 2));
            price_drops.push(json!({
                "asin": asin,
                "name": name_by_asin.get(asin).filter(|n| js_truthy(Some(n))).cloned().unwrap_or(json!(asin)),
                "paid": js::jnum(paid),
                "listing": js::jnum(listing),
                "savings": js::jnum(savings),
                "pct": js::jnum(js::math_round(((paid - listing) / paid) * 100.0)),
            }));
        }
        price_drops.sort_by(|a, b| {
            let d = b["savings"].as_f64().unwrap_or(0.0) - a["savings"].as_f64().unwrap_or(0.0);
            d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        for o in orders.iter_mut() {
            let order_id = o.get("orderId").cloned().unwrap_or(Value::Null);
            for it in o["items"].as_array_mut().into_iter().flatten() {
                let a = asin_of(it);
                if let Some((_, e)) = missing_state.iter().find(|(k, _)| *k == a) {
                    if e.confirmed {
                        it.as_object_mut().unwrap().insert("reviewMissing".into(), json!(true));
                        missing_reviews.push(json!({ "asin": a, "name": it.get("name").cloned().unwrap_or(Value::Null), "orderId": order_id, "since": js::jnum(e.since) }));
                    }
                }
            }
        }
        generated_at = now_ms();
        if cutoff_days > 0 {
            let cutoff_ms = cutoff_days as f64 * DAY_MS;
            for o in orders.iter_mut() {
                let date = o.get("date").and_then(|d| d.as_str()).map(String::from).unwrap_or_default();
                let beyond = notes::parse_order_date(&date).map(|t| (generated_at as f64 - t) > cutoff_ms).unwrap_or(false);
                for it in o["items"].as_array_mut().into_iter().flatten() {
                    let posted = it.get("posted") == Some(&Value::Bool(true));
                    it.as_object_mut().unwrap().insert("beyondCutoff".into(), json!(beyond && !posted));
                }
            }
        }
    });
    let usage = gemini::load_usage(&store.notes_path);
    price_drops.truncate(10);
    let mut out = Map::new();
    out.insert("version".into(), json!(VERSION));
    out.insert("generatedAt".into(), json!(generated_at));
    out.insert("geminiConfigured".into(), json!(!store.gemini_api_key.is_empty()));
    out.insert(
        "gemini".into(),
        json!({ "used": usage.used, "limit": store.gemini_daily_limit, "remaining": (store.gemini_daily_limit - usage.used).max(0), "day": usage.day }),
    );
    out.insert("syncReminderDays".into(), json!(store.sync_reminder_days));
    out.insert("reviewCutoffDays".into(), json!(cutoff_days));
    out.insert("lastSyncAt".into(), last_sync_at.map(js::jnum).unwrap_or(Value::Null));
    out.insert("headless".into(), headless.meta());
    out.insert("priceDrops".into(), Value::Array(price_drops));
    out.insert("missingReviews".into(), Value::Array(missing_reviews));
    out.insert("orders".into(), Value::Array(orders));
    Resp::json(200, &Value::Object(out))
}

/// `String(item.asin || '')`
fn asin_of(it: &Value) -> String {
    let v = it.get("asin");
    if js_truthy(v) {
        to_str(v)
    } else {
        String::new()
    }
}

/// Posted items with stale helpful votes, and unposted items from orders
/// over 2 days old, oldest-stale first; unconfirmed removal suspects first.
pub fn compute_review_check_queue(store: &Store, headless: &Headless, interval_days: i64, now: f64) -> Vec<String> {
    let orders = store.scan_notes();
    let state = headless.state();
    let stale_ms = interval_days as f64 * DAY_MS;
    let mut seen: HashSet<String> = HashSet::new();
    let mut priority = Vec::new();
    let mut entries: Vec<(String, f64)> = Vec::new();
    let mut present = HashSet::new();
    for o in &orders {
        for it in o["items"].as_array().into_iter().flatten() {
            let a = asin_of(it);
            if !a.is_empty() {
                present.insert(a);
            }
        }
    }
    let mut suspects: Vec<&(String, crate::headless::Missing)> =
        state.review_missing.iter().filter(|(_, e)| e.sightings > 0 && !e.confirmed).collect();
    suspects.sort_by(|a, b| (a.1.since - b.1.since).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    for (a, _) in suspects {
        if !present.contains(a) || seen.contains(a) {
            continue;
        }
        seen.insert(a.clone());
        priority.push(a.clone());
    }
    for o in &orders {
        let date = o.get("date").and_then(|d| d.as_str()).unwrap_or("");
        let age = notes::parse_order_date(date).map(|t| now - t);
        for it in o["items"].as_array().into_iter().flatten() {
            let a = asin_of(it);
            if a.is_empty() || seen.contains(&a) {
                continue;
            }
            if it.get("posted") == Some(&Value::Bool(true)) {
                let a_age = state.vote_ts(&a).map(|t| now - t).unwrap_or(9_007_199_254_740_991.0);
                if a_age > stale_ms {
                    seen.insert(a.clone());
                    entries.push((a, a_age));
                }
            } else if let Some(oa) = age.filter(|oa| *oa > 2.0 * DAY_MS) {
                let cutoff = store.review_cutoff_days;
                if cutoff > 0 && oa > cutoff as f64 * DAY_MS {
                    continue;
                }
                seen.insert(a.clone());
                entries.push((a, oa));
            }
        }
    }
    entries.sort_by(|a, b| (b.1 - a.1).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    priority.into_iter().chain(entries.into_iter().map(|e| e.0)).take(REVIEW_CHECK_QUEUE_CAP).collect()
}

pub fn prune_vote_timestamps(store: &Store, headless: &Headless) {
    let orders = store.scan_notes();
    let mut present = HashSet::new();
    for o in &orders {
        for it in o["items"].as_array().into_iter().flatten() {
            let a = asin_of(it);
            if !a.is_empty() {
                present.insert(a);
            }
        }
    }
    headless.prune_vote_timestamps(&present);
}

/// Bind 127.0.0.1:port and serve on a thread per request until `stop`.
pub fn serve(app: Arc<App>, server: Arc<tiny_http::Server>) {
    for request in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || handle(&app, request));
    }
    log!("HTTP server stopped");
}

/// The JS error class a message would carry in Node's log line.
fn ename(msg: &str) -> &'static str {
    if msg.starts_with("Cannot ") {
        "TypeError"
    } else {
        "Error"
    }
}

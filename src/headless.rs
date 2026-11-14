//! Headless auto-sync: a Chromium-family browser with the extension loaded,
//! run against a dedicated profile, plus its persisted state file
//! (<notesPath>/.headless-sync.json), run history and the review checker.

use crate::config::{expand_home, resolve, DEFAULT_AUTOSYNC_INTERVAL_HOURS};
use crate::js::{self, json_is_int, json_num};
use crate::jsdate::now_ms;
use crate::notes::{self, guidance_for};
use crate::store::{self, read_text};
use crate::{elog, log};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const HEADLESS_TIMEOUT_MS: u64 = 6 * 60 * 1000;
const HEADLESS_HISTORY_CAP: usize = 20;
const STALL_WATCHDOG_DEFAULT_MS: u64 = 45 * 1000;
const WAIT_TICK_DEFAULT_MS: u64 = 15 * 1000;

// ---------------------------------------------------------------------------
// Run options
// ---------------------------------------------------------------------------

pub fn default_run_options() -> Value {
    json!({ "mode": "full", "maxOrders": 0, "timeBudgetSec": 0, "refresh": { "prices": true, "votes": true, "details": true } })
}

pub fn normalize_run_options(body: Option<&Value>) -> Value {
    let mut o = default_run_options();
    let Some(Value::Object(b)) = body else { return o };
    if let Some(Value::String(m)) = b.get("mode") {
        if m == "quick" || m == "full" || m == "catchup" {
            o["mode"] = json!(m);
        }
    }
    if let Some(n) = json_is_int(b.get("maxOrders")).filter(|n| *n >= 0) {
        o["maxOrders"] = json!(n);
    }
    if let Some(n) = json_is_int(b.get("timeBudgetSec")).filter(|n| *n >= 0) {
        o["timeBudgetSec"] = json!(n);
    }
    if let Some(Value::Object(r)) = b.get("refresh") {
        for k in ["prices", "votes", "details"] {
            if let Some(Value::Bool(v)) = r.get(k) {
                o["refresh"][k] = json!(v);
            }
        }
    }
    o
}

// ---------------------------------------------------------------------------
// Browser discovery
// ---------------------------------------------------------------------------

const BROWSER_CANDIDATES: [&str; 10] = [
    "chromium",
    "chromium-browser",
    "brave-browser",
    "brave",
    "brave-origin",
    "microsoft-edge",
    "msedge",
    "google-chrome",
    "google-chrome-stable",
    "chrome",
];

pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
        unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
    }
    #[cfg(windows)]
    {
        // Node's X_OK check on Windows is an existence check.
        std::fs::metadata(p).is_ok()
    }
}

pub fn windows_browser_candidates_with(env: crate::config::Env) -> Vec<PathBuf> {
    let mut locs: Vec<(&str, Vec<&str>)> = Vec::new();
    for ch in ["Brave-Browser", "Brave-Origin", "Brave-Browser-Beta", "Brave-Browser-Nightly"] {
        for base in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
            locs.push((base, vec!["BraveSoftware", ch, "Application", "brave.exe"]));
        }
    }
    locs.push(("ProgramFiles(x86)", vec!["Microsoft", "Edge", "Application", "msedge.exe"]));
    locs.push(("ProgramFiles", vec!["Microsoft", "Edge", "Application", "msedge.exe"]));
    locs.push(("LOCALAPPDATA", vec!["Chromium", "Application", "chrome.exe"]));
    locs.push(("ProgramFiles", vec!["Google", "Chrome", "Application", "chrome.exe"]));
    locs.push(("ProgramFiles(x86)", vec!["Google", "Chrome", "Application", "chrome.exe"]));
    locs.push(("LOCALAPPDATA", vec!["Google", "Chrome", "Application", "chrome.exe"]));
    let mut out = Vec::new();
    for (var, segs) in locs {
        if let Some(base) = env(var) {
            let mut p = PathBuf::from(base);
            for s in segs {
                p.push(s);
            }
            out.push(p);
        }
    }
    out
}

pub fn find_browser_binary(browser_path: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    find_browser_binary_with(browser_path, &path_env, cfg!(windows), &crate::config::real_env)
}

/// browserPath when it is executable, else the first candidate on PATH, then
/// (Windows) the usual install folders.
pub fn find_browser_binary_with(browser_path: &str, path_env: &std::ffi::OsStr, windows: bool, env: crate::config::Env) -> Option<PathBuf> {
    let bp = js::trim(browser_path);
    if !bp.is_empty() {
        let configured = resolve(&expand_home(bp));
        if is_executable(&configured) {
            return Some(configured);
        }
    }
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in BROWSER_CANDIDATES {
            let cand = dir.join(if windows { format!("{}.exe", name) } else { name.to_string() });
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    if windows {
        for c in windows_browser_candidates_with(env) {
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

struct BrowserHit {
    resolved: Option<PathBuf>,
    at: i64,
}

fn browser_cache() -> &'static Mutex<HashMap<String, BrowserHit>> {
    static C: OnceLock<Mutex<HashMap<String, BrowserHit>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

static TTL_OVERRIDE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[cfg(all(test, unix))]
pub fn set_browser_cache_ttl_for_tests(ms: Option<i64>) {
    TTL_OVERRIDE.store(ms.unwrap_or(-1), std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub fn reset_browser_cache_for_tests() {
    browser_cache().lock().unwrap().clear();
}

fn browser_cache_ttl_ms() -> i64 {
    let o = TTL_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst);
    if o >= 0 {
        return o;
    }
    std::env::var("AOS_BROWSER_CACHE_TTL_MS").ok().and_then(|v| v.parse::<i64>().ok()).filter(|v| *v >= 0).unwrap_or(60_000)
}

/// The companion extension: AOS_EXTENSION_DIR, else browser-extension/ next
/// to the program or in a folder above it (a cargo build in the repo).
pub fn extension_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AOS_EXTENSION_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok().or(Some(p)));
    let mut dir = exe.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
    let first = dir.clone();
    for _ in 0..5 {
        let Some(d) = dir else { break };
        let cand = d.join("browser-extension");
        if cand.join("manifest.json").is_file() {
            return strip_verbatim(cand);
        }
        // An installed layout: <prefix>/bin/amazon-order-sync and
        // <prefix>/share/amazon-order-sync/browser-extension.
        let share = d.join("share").join("amazon-order-sync").join("browser-extension");
        if share.join("manifest.json").is_file() {
            return strip_verbatim(share);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    first.unwrap_or_else(|| PathBuf::from(".")).join("browser-extension")
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if !rest.starts_with("UNC\\") {
            return PathBuf::from(rest);
        }
    }
    p
}

// ---------------------------------------------------------------------------
// Pure predicates
// ---------------------------------------------------------------------------

pub fn is_login_blocked(state: &State) -> bool {
    state.last_result.as_deref() == Some("error")
        && state.last_error.as_deref().map(|e| e.to_lowercase().contains("login")).unwrap_or(false)
}

pub fn is_due(
    enabled: bool,
    browser_found: bool,
    running: bool,
    login_blocked: bool,
    last: Option<f64>,
    interval_ms: f64,
    now: f64,
) -> bool {
    if !enabled || !browser_found || running || login_blocked {
        return false;
    }
    match last {
        None => true,
        Some(l) => (now - l) > interval_ms,
    }
}

pub fn is_pid_alive(pid: i64) -> bool {
    if pid <= 0 || pid > u32::MAX as i64 {
        return false;
    }
    #[cfg(unix)]
    {
        if pid > i32::MAX as i64 {
            return false;
        }
        let r = unsafe { libc::kill(pid as i32, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        crate::winproc::pid_alive(pid as u32)
    }
}

// ---------------------------------------------------------------------------
// Persisted state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct HistoryEntry {
    pub t: f64,
    pub result: String,
    pub error: Option<String>,
    pub orders_synced: i64,
    pub duration_ms: f64,
}

#[derive(Clone, Debug)]
pub struct Missing {
    pub since: f64,
    pub sightings: i64,
    pub confirmed: bool,
    pub notified: bool,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub last_run_at: Option<f64>,
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    pub orders_synced: i64,
    pub history: Vec<HistoryEntry>,
    pub vote_timestamps: Vec<(String, f64)>,
    pub last_review_check_at: Option<f64>,
    pub last_options: Option<Value>,
    pub review_missing: Vec<(String, Missing)>,
}

fn opt_num(v: Option<f64>) -> Value {
    v.map(js::jnum).unwrap_or(Value::Null)
}

fn opt_str(v: &Option<String>) -> Value {
    v.as_ref().map(|s| json!(s)).unwrap_or(Value::Null)
}

impl HistoryEntry {
    fn to_json(&self) -> Value {
        json!({
            "t": js::jnum(self.t),
            "result": self.result,
            "error": opt_str(&self.error),
            "ordersSynced": self.orders_synced,
            "durationMs": js::jnum(self.duration_ms),
        })
    }
}

impl State {
    pub fn to_json(&self) -> Value {
        let mut vt = Map::new();
        for (k, v) in &self.vote_timestamps {
            vt.insert(k.clone(), js::jnum(*v));
        }
        let mut rm = Map::new();
        for (k, m) in &self.review_missing {
            rm.insert(
                k.clone(),
                json!({ "since": js::jnum(m.since), "sightings": m.sightings, "confirmed": m.confirmed, "notified": m.notified }),
            );
        }
        let mut o = Map::new();
        o.insert("lastRunAt".into(), opt_num(self.last_run_at));
        o.insert("lastResult".into(), opt_str(&self.last_result));
        o.insert("lastError".into(), opt_str(&self.last_error));
        o.insert("ordersSynced".into(), json!(self.orders_synced));
        o.insert("history".into(), Value::Array(self.history.iter().map(HistoryEntry::to_json).collect()));
        o.insert("voteTimestamps".into(), Value::Object(vt));
        o.insert("lastReviewCheckAt".into(), opt_num(self.last_review_check_at));
        o.insert("lastOptions".into(), self.last_options.clone().unwrap_or(Value::Null));
        o.insert("reviewMissing".into(), Value::Object(rm));
        Value::Object(o)
    }

    pub fn vote_ts(&self, asin: &str) -> Option<f64> {
        self.vote_timestamps.iter().find(|(a, _)| a == asin).map(|(_, t)| *t)
    }

    fn set_vote_ts(&mut self, asin: &str, t: f64) {
        if let Some(e) = self.vote_timestamps.iter_mut().find(|(a, _)| a == asin) {
            e.1 = t;
        } else {
            self.vote_timestamps.push((asin.to_string(), t));
            js_order_pairs(&mut self.vote_timestamps);
        }
    }

    pub fn missing(&self, asin: &str) -> Option<&Missing> {
        self.review_missing.iter().find(|(a, _)| a == asin).map(|(_, m)| m)
    }
}

/// Keep (key, value) pairs in JS object order (array-index keys first).
fn js_order_pairs<T: Clone>(v: &mut Vec<(String, T)>) {
    let mut m = Map::new();
    for (i, (k, _)) in v.iter().enumerate() {
        m.insert(k.clone(), json!(i));
    }
    js::order_object(&mut m);
    let old = v.clone();
    *v = m.values().map(|i| old[i.as_u64().unwrap() as usize].clone()).collect();
}

fn valid_result(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) if s == "ok" || s == "error" || s == "cancelled" => Some(s.clone()),
        _ => None,
    }
}

fn parse_state(parsed: &Value, state: &mut State) {
    let Value::Object(p) = parsed else { return };
    state.last_run_at = json_num(p.get("lastRunAt")).filter(|n| *n > 0.0);
    state.last_result = valid_result(p.get("lastResult"));
    state.last_error = p.get("lastError").and_then(|v| v.as_str()).map(String::from);
    state.orders_synced = json_is_int(p.get("ordersSynced")).filter(|n| *n >= 0).unwrap_or(0);
    state.history = p
        .get("history")
        .and_then(|h| h.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    let o = h.as_object()?;
                    let t = json_num(o.get("t")).filter(|t| *t > 0.0)?;
                    let result = valid_result(o.get("result"))?;
                    let orders = json_is_int(o.get("ordersSynced")).filter(|n| *n >= 0)?;
                    let dur = json_num(o.get("durationMs")).filter(|d| *d >= 0.0)?;
                    Some(HistoryEntry {
                        t,
                        result,
                        error: o.get("error").and_then(|e| e.as_str()).map(String::from),
                        orders_synced: orders,
                        duration_ms: dur,
                    })
                })
                .take(HEADLESS_HISTORY_CAP)
                .collect()
        })
        .unwrap_or_default();
    state.vote_timestamps = Vec::new();
    if let Some(Value::Object(vt)) = p.get("voteTimestamps") {
        for (asin, t) in vt {
            if let Some(t) = json_num(Some(t)).filter(|t| *t > 0.0) {
                if notes::context_asin_ok(asin) {
                    state.vote_timestamps.push((asin.clone(), t));
                }
            }
        }
    }
    state.last_review_check_at = json_num(p.get("lastReviewCheckAt")).filter(|n| *n > 0.0);
    state.last_options = match p.get("lastOptions") {
        Some(v @ Value::Object(_)) => Some(normalize_run_options(Some(v))),
        _ => None,
    };
    state.review_missing = Vec::new();
    if let Some(Value::Object(rm)) = p.get("reviewMissing") {
        for (asin, e) in rm {
            let Some(o) = e.as_object() else { continue };
            if !notes::context_asin_ok(asin) {
                continue;
            }
            let Some(since) = json_num(o.get("since")).filter(|s| *s > 0.0) else { continue };
            let Some(sightings) = json_is_int(o.get("sightings")).filter(|s| *s > 0) else { continue };
            state.review_missing.push((
                asin.clone(),
                Missing {
                    since,
                    sightings,
                    confirmed: o.get("confirmed") == Some(&Value::Bool(true)),
                    notified: o.get("notified") == Some(&Value::Bool(true)),
                },
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Notifications (desktop): notify-send on Linux; the tray polls on Windows.
// ---------------------------------------------------------------------------

static NOTIFY_SEND: OnceLock<Option<PathBuf>> = OnceLock::new();

fn resolve_notify_send() -> Option<PathBuf> {
    NOTIFY_SEND
        .get_or_init(|| {
            if let Ok(o) = std::env::var("AOS_NOTIFY_SEND") {
                if !o.is_empty() && is_executable(Path::new(&o)) {
                    return Some(PathBuf::from(o));
                }
            }
            let path_env = std::env::var_os("PATH").unwrap_or_default();
            for dir in std::env::split_paths(&path_env) {
                if dir.as_os_str().is_empty() {
                    continue;
                }
                let c = dir.join("notify-send");
                if is_executable(&c) {
                    return Some(c);
                }
            }
            None
        })
        .clone()
}

pub fn try_notify(title: &str, body: &str, enabled: bool) {
    if !enabled {
        return;
    }
    crate::logbuf::push_notification(title, body);
    if cfg!(windows) {
        return;
    }
    let Some(bin) = resolve_notify_send() else { return };
    if let Ok(mut c) = Command::new(bin).arg(title).arg(body).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || c.wait());
    }
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn detached(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

/// Kill a detached child's whole tree: SIGTERM to the group, SIGKILL after
/// 5 s (taskkill /T /F on Windows), and reap it.
fn kill_process_group(mut child: Child, wait_for_kill: bool) {
    let pid = child.id();
    #[cfg(windows)]
    {
        let mut k = Command::new("taskkill");
        k.args(["/pid", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        crate::winproc::no_window(&mut k);
        let launched = if wait_for_kill { k.status().is_ok() } else { k.spawn().map(|mut t| std::thread::spawn(move || t.wait())).is_ok() };
        if !launched {
            let _ = child.kill();
        }
        std::thread::spawn(move || child.wait());
    }
    #[cfg(unix)]
    {
        let _ = wait_for_kill;
        let r = unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
        if r != 0 {
            let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if Instant::now() >= deadline {
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Outcome {
    pub last_result: String,
    pub last_error: Option<String>,
    pub orders_synced: i64,
}

impl Outcome {
    fn err(e: &str, n: i64) -> Outcome {
        Outcome { last_result: "error".into(), last_error: Some(e.into()), orders_synced: n }
    }
}

pub fn result_json(o: &Outcome) -> Value {
    let mut m = Map::new();
    m.insert("lastResult".into(), json!(o.last_result));
    m.insert("lastError".into(), opt_str(&o.last_error));
    m.insert("ordersSynced".into(), json!(o.orders_synced));
    if let Some(e) = o.last_error.as_ref().filter(|e| !e.is_empty()) {
        m.insert("guidance".into(), guidance_for(e));
    }
    Value::Object(m)
}

pub enum RunResult {
    Already,
    NoBrowser(Value),
    Done(Value),
}

struct Inner {
    running: bool,
    state: State,
    /// A run is waiting for its outcome (the completion-ping await).
    pending: bool,
    outcome: Option<Outcome>,
    active_child: Option<u32>,
    run_options: Option<Value>,
    run_started_at: Option<i64>,
    client_log_lines: i64,
    orders_posted: i64,
    extension_alive_logged: bool,
    login_skip_logged_day: Option<String>,
    setup_child: Option<u64>,
    setup_seq: u64,
}

pub struct Headless {
    pub notes_path: PathBuf,
    pub enabled: bool,
    pub interval_hours: i64,
    pub browser_path: String,
    pub notifications_enabled: bool,
    pub profile_dir: PathBuf,
    pub stall_watchdog_ms: u64,
    pub wait_tick_ms: u64,
    inner: Mutex<Inner>,
    cond: Condvar,
    pub(crate) state_save_delay_once: Mutex<u64>,
}

fn env_pos_ms(name: &str) -> Option<u64> {
    let n = js::string_to_number(&std::env::var(name).ok()?);
    if js::is_integer(n) && n > 0.0 {
        Some(n as u64)
    } else {
        None
    }
}

impl Headless {
    pub fn new(
        notes_path: PathBuf,
        enabled: bool,
        interval_hours: i64,
        browser_path: String,
        notifications_enabled: bool,
    ) -> Arc<Headless> {
        let profile_dir = match std::env::var("AOS_PROFILE_DIR") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ => crate::config::default_data_dir().join("browser-profile"),
        };
        Arc::new(Headless {
            notes_path,
            enabled,
            interval_hours: if interval_hours > 0 { interval_hours } else { DEFAULT_AUTOSYNC_INTERVAL_HOURS },
            browser_path,
            notifications_enabled,
            profile_dir,
            stall_watchdog_ms: env_pos_ms("AOS_STALL_WATCHDOG_MS").unwrap_or(STALL_WATCHDOG_DEFAULT_MS),
            wait_tick_ms: env_pos_ms("AOS_WAIT_TICK_MS").unwrap_or(WAIT_TICK_DEFAULT_MS),
            inner: Mutex::new(Inner {
                running: false,
                state: State::default(),
                pending: false,
                outcome: None,
                active_child: None,
                run_options: None,
                run_started_at: None,
                client_log_lines: 0,
                orders_posted: 0,
                extension_alive_logged: false,
                login_skip_logged_day: None,
                setup_child: None,
                setup_seq: 0,
            }),
            cond: Condvar::new(),
            state_save_delay_once: Mutex::new(env_pos_ms("AOS_STATE_SAVE_DELAY_ONCE_MS").unwrap_or(0)),
        })
    }

    fn state_file(&self) -> PathBuf {
        self.notes_path.join(".headless-sync.json")
    }

    pub fn load_state(&self) {
        let Ok(txt) = read_text(&self.state_file()) else { return };
        let Some(v) = js::json_parse(&txt) else { return };
        let mut g = self.inner.lock().unwrap();
        parse_state(&v, &mut g.state);
    }

    fn save_state_locked(&self, state: &State) {
        let body = js::stringify_pretty(&state.to_json()) + "\n";
        let file = self.state_file();
        let tmp = PathBuf::from(format!("{}{}", file.display(), store::tmp_suffix()));
        let res = std::fs::write(&tmp, body.as_bytes()).and_then(|_| {
            let delay = std::mem::take(&mut *self.state_save_delay_once.lock().unwrap());
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            std::fs::rename(&tmp, &file)
        });
        if let Err(e) = res {
            elog!("Failed to persist headless sync state: {}", e);
        }
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn running(&self) -> bool {
        self.inner.lock().unwrap().running
    }

    /// Test seam: the inconsistent "running with nothing pending" state.
    #[cfg(test)]
    pub fn force_stuck_for_tests(&self, pending: bool) {
        let mut g = self.inner.lock().unwrap();
        g.running = true;
        g.pending = pending;
        g.outcome = None;
        g.active_child = None;
        g.run_started_at = Some(now_ms());
        g.run_options = Some(json!({ "mode": "full" }));
    }

    #[cfg(test)]
    pub fn take_outcome_for_tests(&self) -> Option<Outcome> {
        let mut g = self.inner.lock().unwrap();
        g.running = false;
        g.outcome.take()
    }

    #[cfg(test)]
    pub fn run_started_at_for_tests(&self) -> Option<i64> {
        self.inner.lock().unwrap().run_started_at
    }

    /// A run is waiting for its completion ping.
    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.inner.lock().unwrap().pending
    }

    #[cfg(test)]
    pub fn new_for_tests(
        notes_path: PathBuf,
        enabled: bool,
        browser_path: &str,
        notifications_enabled: bool,
        profile_dir: PathBuf,
    ) -> Arc<Headless> {
        let h = Headless::new(notes_path, enabled, 24, browser_path.to_string(), notifications_enabled);
        let mut h = Arc::try_unwrap(h).ok().expect("fresh");
        h.profile_dir = profile_dir;
        Arc::new(h)
    }

    pub fn login_blocked(&self) -> bool {
        is_login_blocked(&self.inner.lock().unwrap().state)
    }

    pub fn setup_browser_open(&self) -> bool {
        self.inner.lock().unwrap().setup_child.is_some()
    }

    pub fn resolve_browser(&self) -> Option<PathBuf> {
        let key = self.browser_path.clone();
        let mut cache = browser_cache().lock().unwrap();
        if let Some(hit) = cache.get_mut(&key) {
            let now = now_ms();
            if now - hit.at < browser_cache_ttl_ms() {
                return hit.resolved.clone();
            }
            if let Some(r) = hit.resolved.clone() {
                if is_executable(&r) {
                    hit.at = now;
                    return Some(r);
                }
            }
            cache.remove(&key);
        }
        let resolved = find_browser_binary(&self.browser_path);
        if let Some(r) = &resolved {
            let s = r.to_string_lossy();
            let chrome = js::test(js::re(r"google[\\/ -]?chrome|[\\/]chrome(\.exe)?$", "i"), &s);
            if chrome && !s.to_lowercase().contains("chromium") {
                log!(
                    "Browser for headless runs is Google Chrome ({}); it ignores --load-extension since Chrome 137, so runs will stall. Install Brave, Edge or Chromium, or set browserPath.",
                    s
                );
            }
        }
        cache.insert(key, BrowserHit { resolved: resolved.clone(), at: now_ms() });
        resolved
    }

    fn invalidate_browser_cache(&self) {
        browser_cache().lock().unwrap().remove(&self.browser_path);
    }

    pub fn meta(&self) -> Value {
        let browser = self.resolve_browser();
        let g = self.inner.lock().unwrap();
        let s = &g.state;
        let mut m = Map::new();
        m.insert("enabled".into(), json!(self.enabled));
        m.insert("running".into(), json!(g.running));
        m.insert("loginBlocked".into(), json!(is_login_blocked(s)));
        m.insert("setupOpen".into(), json!(g.setup_child.is_some()));
        m.insert("runStartedAt".into(), if g.running { g.run_started_at.map(|t| json!(t)).unwrap_or(Value::Null) } else { Value::Null });
        m.insert("lastRunAt".into(), opt_num(s.last_run_at));
        m.insert("lastResult".into(), opt_str(&s.last_result));
        m.insert("lastError".into(), opt_str(&s.last_error));
        m.insert("ordersSynced".into(), json!(s.orders_synced));
        m.insert("intervalHours".into(), json!(self.interval_hours));
        m.insert("browserFound".into(), json!(browser.is_some()));
        m.insert("profileDir".into(), json!(self.profile_dir.to_string_lossy()));
        m.insert("browserBinary".into(), browser.map(|b| json!(b.to_string_lossy())).unwrap_or(Value::Null));
        m.insert("history".into(), Value::Array(s.history.iter().take(10).map(HistoryEntry::to_json).collect()));
        m.insert("lastReviewCheckAt".into(), opt_num(s.last_review_check_at));
        m.insert("lastOptions".into(), s.last_options.clone().unwrap_or(Value::Null));
        m.insert("guidance".into(), s.last_error.as_ref().filter(|e| !e.is_empty()).map(|e| guidance_for(e)).unwrap_or(Value::Null));
        Value::Object(m)
    }

    pub fn run_options(&self) -> Value {
        let g = self.inner.lock().unwrap();
        g.run_options.clone().or_else(|| g.state.last_options.clone()).unwrap_or_else(default_run_options)
    }

    fn push_history(state: &mut State, e: HistoryEntry) {
        state.history.insert(0, e);
        state.history.truncate(HEADLESS_HISTORY_CAP);
    }

    fn notify_outcome(&self, o: &Outcome) {
        if o.last_result == "ok" {
            if o.orders_synced > 0 {
                try_notify("Amazon Order Sync", &format!("{} orders synced", o.orders_synced), self.notifications_enabled);
            }
        } else if o.last_result == "error" {
            let e = o.last_error.clone().unwrap_or_else(|| "null".into());
            try_notify("Amazon Order Sync failed", &e, self.notifications_enabled);
        }
    }

    pub fn bump_vote_timestamps(&self, asins: &[String]) {
        let now = now_ms() as f64;
        let mut g = self.inner.lock().unwrap();
        let mut changed = false;
        for a in asins {
            if !notes::context_asin_ok(a) {
                continue;
            }
            g.state.set_vote_ts(a, now);
            changed = true;
        }
        if changed {
            self.save_state_locked(&g.state);
        }
    }

    /// Two consecutive loaded-empty sightings confirm a removed review.
    pub fn record_missing_sighting(&self, asin: &str, item_name: &str) {
        let now = now_ms() as f64;
        let mut g = self.inner.lock().unwrap();
        let mut entry = g.state.missing(asin).cloned().unwrap_or(Missing { since: now, sightings: 0, confirmed: false, notified: false });
        entry.sightings += 1;
        if entry.sightings >= 2 {
            entry.confirmed = true;
        }
        let notify = entry.confirmed && !entry.notified;
        if notify {
            entry.notified = true;
        }
        if let Some(e) = g.state.review_missing.iter_mut().find(|(a, _)| a == asin) {
            e.1 = entry;
        } else {
            g.state.review_missing.push((asin.to_string(), entry));
            js_order_pairs(&mut g.state.review_missing);
        }
        if notify {
            try_notify("Amazon review missing", &format!("{} — your posted review is gone", item_name), self.notifications_enabled);
        }
        self.save_state_locked(&g.state);
    }

    pub fn clear_missing_review(&self, asin: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(i) = g.state.review_missing.iter().position(|(a, _)| a == asin) else { return false };
        g.state.review_missing.remove(i);
        self.save_state_locked(&g.state);
        true
    }

    pub fn note_client_log_lines(&self, n: i64) {
        let mut g = self.inner.lock().unwrap();
        g.client_log_lines += if n > 0 { n } else { 1 };
        if g.running && !g.extension_alive_logged {
            g.extension_alive_logged = true;
            drop(g);
            log!("Extension alive in the sync browser.");
        }
    }

    pub fn note_order_posted(&self) {
        self.inner.lock().unwrap().orders_posted += 1;
    }

    fn settle(&self, g: &mut Inner, o: Outcome) {
        if g.pending && g.outcome.is_none() {
            g.outcome = Some(o);
            g.pending = false;
            self.cond.notify_all();
        }
    }

    /// One lifecycle for the orders sync and the review checker. Blocks
    /// until the run finishes (completion ping, stop, stall, exit, timeout).
    pub fn run_browser(&self, url: &str, review_check: bool, options: Option<Value>) -> RunResult {
        let binary = {
            if self.inner.lock().unwrap().running {
                return RunResult::Already;
            }
            self.resolve_browser()
        };
        let mut g = self.inner.lock().unwrap();
        if g.running {
            return RunResult::Already;
        }
        let Some(binary) = binary else {
            let o = Outcome::err("no-browser", 0);
            let now = now_ms() as f64;
            g.state.last_run_at = Some(now);
            g.state.last_result = Some(o.last_result.clone());
            g.state.last_error = o.last_error.clone();
            g.state.orders_synced = 0;
            if review_check {
                g.state.last_review_check_at = Some(now_ms() as f64);
            }
            self.save_state_locked(&g.state);
            drop(g);
            self.notify_outcome(&o);
            return RunResult::NoBrowser(result_json(&o));
        };

        g.running = true;
        let started = now_ms();
        let started_i = Instant::now();
        g.run_started_at = Some(started);
        g.client_log_lines = 0;
        g.orders_posted = 0;
        g.extension_alive_logged = false;
        g.run_options = if !review_check { options.clone() } else { None };
        g.pending = true;
        g.outcome = None;
        let setup_open = g.setup_child.is_some();
        let run_opts = g.run_options.clone();
        drop(g);
        if !review_check {
            crate::gemini::reset_quota_log_marks();
        }
        if review_check {
            log!("Headless review-check started.");
        } else {
            let o = run_opts.unwrap_or_else(default_run_options);
            let max = o["maxOrders"].as_i64().unwrap_or(0);
            let budget = o["timeBudgetSec"].as_i64().unwrap_or(0);
            log!(
                "Headless sync started (mode={}, maxOrders={}, budget={}s, refresh=prices:{} votes:{} details:{})",
                o["mode"].as_str().unwrap_or("full"),
                if max == 0 { "all".to_string() } else { max.to_string() },
                if budget == 0 { "none".to_string() } else { budget.to_string() },
                o["refresh"]["prices"],
                o["refresh"]["votes"],
                o["refresh"]["details"]
            );
        }

        let run = || -> Result<Option<Child>, String> {
            std::fs::create_dir_all(&self.profile_dir).map_err(|e| e.to_string())?;
            let mut removed = Vec::new();
            let mut skip = setup_open;
            if !skip {
                if let Ok(target) = std::fs::read_link(self.profile_dir.join("SingletonLock")) {
                    let t = target.to_string_lossy();
                    if let Some(m) = js::find(js::re(r"-(\d+)$", ""), &t) {
                        let pid: i64 = m.group(&t, 1).unwrap_or("0").parse().unwrap_or(0);
                        if is_pid_alive(pid) {
                            skip = true;
                            log!("Browser profile SingletonLock owner pid {} is alive — leaving profile locks untouched.", pid);
                        }
                    }
                }
            }
            if !skip {
                for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
                    let p = self.profile_dir.join(name);
                    if std::fs::symlink_metadata(&p).is_err() {
                        continue;
                    }
                    let ok = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir_all(&p));
                    if ok.is_ok() {
                        removed.push(name);
                    }
                }
            }
            if !removed.is_empty() {
                log!("Removed stale browser profile lock(s): {}", removed.join(", "));
            }
            let ext = extension_dir();
            let mut cmd = Command::new(&binary);
            cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check"])
                .arg(format!("--user-data-dir={}", self.profile_dir.display()))
                .arg(format!("--disable-extensions-except={}", ext.display()))
                .arg(format!("--load-extension={}", ext.display()))
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            detached(&mut cmd);
            match cmd.spawn() {
                Ok(c) => Ok(Some(c)),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        self.invalidate_browser_cache();
                    }
                    let mut g = self.inner.lock().unwrap();
                    self.settle(&mut g, Outcome::err("browser failed to start", 0));
                    Ok(None)
                }
            }
        };

        let spawned = run();
        let mut child: Option<Child> = None;
        let outcome = match spawned {
            Err(msg) => {
                let n = self.inner.lock().unwrap().orders_posted;
                Err(Outcome::err(&msg, n))
            }
            Ok(c) => {
                child = c;
                if let Some(c) = &child {
                    self.inner.lock().unwrap().active_child = Some(c.id());
                }
                Ok(self.wait_outcome(&mut child, started_i, started))
            }
        };
        let o = match outcome {
            Ok(o) | Err(o) => o,
        };

        let finished = now_ms();
        let result = result_json(&o);
        log!(
            "Headless run finished: {}, {} orders, {}s",
            o.last_result,
            o.orders_synced,
            js::num_to_string(js::math_round((finished - started) as f64 / 1000.0))
        );
        {
            let mut g = self.inner.lock().unwrap();
            if !review_check {
                if let Some(opts) = &options {
                    g.state.last_options = Some(opts.clone());
                }
            }
            g.state.last_run_at = Some(finished as f64);
            g.state.last_result = Some(o.last_result.clone());
            g.state.last_error = o.last_error.clone();
            g.state.orders_synced = o.orders_synced;
            if review_check {
                g.state.last_review_check_at = Some(finished as f64);
            }
            Self::push_history(
                &mut g.state,
                HistoryEntry {
                    t: finished as f64,
                    result: o.last_result.clone(),
                    error: o.last_error.clone(),
                    orders_synced: o.orders_synced,
                    duration_ms: (finished - started) as f64,
                },
            );
            self.save_state_locked(&g.state);
        }
        self.notify_outcome(&o);

        // Every exit path kills the browser's process group.
        if let Some(c) = child.take() {
            kill_process_group(c, false);
        }
        let mut g = self.inner.lock().unwrap();
        g.pending = false;
        g.outcome = None;
        g.active_child = None;
        g.run_options = None;
        g.run_started_at = None;
        g.running = false;
        drop(g);
        RunResult::Done(result)
    }

    fn wait_outcome(&self, child: &mut Option<Child>, started: Instant, started_ms: i64) -> Outcome {
        let timeout = Duration::from_millis(HEADLESS_TIMEOUT_MS);
        let watchdog = Duration::from_millis(self.stall_watchdog_ms);
        let tick = Duration::from_millis(self.wait_tick_ms);
        let mut watchdog_armed = true;
        let mut ticks_armed = true;
        let mut exited = false;
        let mut next_tick = started + tick;
        let mut g = self.inner.lock().unwrap();
        loop {
            if let Some(o) = g.outcome.take() {
                return o;
            }
            let now = Instant::now();
            let activity = g.client_log_lines > 0 || g.orders_posted > 0;
            if watchdog_armed && now >= started + watchdog {
                watchdog_armed = false;
                if !activity && g.pending {
                    let secs = js::num_to_string(js::math_round(self.stall_watchdog_ms as f64 / 1000.0));
                    drop(g);
                    log!("Headless run stalled: no extension activity after {}s; aborting.", secs);
                    g = self.inner.lock().unwrap();
                    let msg = format!("browser-stalled — no extension activity after {}s", secs);
                    self.settle(&mut g, Outcome::err(&msg, 0));
                    continue;
                }
            }
            if ticks_armed && now >= next_tick {
                if activity {
                    ticks_armed = false;
                } else {
                    let elapsed = js::num_to_string(js::math_round((now_ms() - started_ms) as f64 / 1000.0));
                    let (c, o) = (g.client_log_lines, g.orders_posted);
                    drop(g);
                    log!("…waiting for extension: {} client lines, {} orders ({}s)", c, o, elapsed);
                    g = self.inner.lock().unwrap();
                    next_tick += tick;
                    continue;
                }
            }
            if now >= started + timeout {
                let n = g.orders_posted;
                self.settle(&mut g, Outcome::err("timeout — probably not logged in; run setup", n));
                continue;
            }
            if let Some(c) = child.as_mut().filter(|_| !exited) {
                if let Ok(Some(status)) = c.try_wait() {
                    exited = true;
                    let how = match status.code() {
                        Some(code) => format!("code {}", code),
                        None => {
                            #[cfg(unix)]
                            {
                                use std::os::unix::process::ExitStatusExt;
                                format!("signal {}", signal_name(status.signal().unwrap_or(0)))
                            }
                            #[cfg(not(unix))]
                            {
                                "signal null".to_string()
                            }
                        }
                    };
                    // Reaped, but its helpers may still hold the group: the
                    // run's exit path still kills the group.
                    drop(g);
                    log!("Browser exited early ({}) — crash or profile issue", how);
                    g = self.inner.lock().unwrap();
                    let n = g.orders_posted;
                    self.settle(&mut g, Outcome::err("browser-exited — the sync browser closed unexpectedly", n));
                    continue;
                }
            }
            let mut next = started + timeout;
            if watchdog_armed {
                next = next.min(started + watchdog);
            }
            if ticks_armed {
                next = next.min(next_tick);
            }
            let wait = next.saturating_duration_since(Instant::now()).min(Duration::from_millis(50));
            g = self.cond.wait_timeout(g, wait).unwrap().0;
        }
    }

    pub fn run_headless_sync(&self, options: Option<&Value>) -> RunResult {
        let url = format!("https://www.amazon.com/your-orders#aos-autosync-{}", now_ms());
        self.run_browser(&url, false, Some(normalize_run_options(options)))
    }

    pub fn run_review_check(&self, first_asin: &str) -> RunResult {
        let url = format!(
            "https://www.amazon.com/review/create-review?asin={}#aos-reviewcheck-{}",
            js::encode_uri_component(first_asin),
            now_ms()
        );
        self.run_browser(&url, true, None)
    }

    /// The extension's completion ping. False when no run is waiting.
    pub fn complete(&self, payload: &Value) -> bool {
        let mut g = self.inner.lock().unwrap();
        if !g.pending {
            return false;
        }
        let ok = notes::prop(payload, "ok") == Some(&Value::Bool(true));
        let processed = json_is_int(notes::prop(payload, "processed")).filter(|n| *n >= 0).unwrap_or(0);
        let o = if ok {
            Outcome { last_result: "ok".into(), last_error: None, orders_synced: processed }
        } else {
            let e = match notes::prop(payload, "error") {
                Some(Value::String(s)) if !s.is_empty() => s.clone(),
                _ => "unknown".into(),
            };
            Outcome::err(&e, 0)
        };
        self.settle(&mut g, o);
        true
    }

    fn clear_stuck(g: &mut Inner) {
        g.running = false;
        g.run_started_at = None;
        g.run_options = None;
    }

    /// Abort the active run. None = idle (409); Some(true) = stopped;
    /// Some(false) = a stuck flag was cleared ("recovered").
    pub fn stop(&self) -> Option<bool> {
        let mut g = self.inner.lock().unwrap();
        if !g.running {
            return None;
        }
        if !g.pending {
            Self::clear_stuck(&mut g);
            drop(g);
            log!("Cleared a stuck run flag via sync-stop");
            return Some(false);
        }
        let n = g.orders_posted;
        self.settle(&mut g, Outcome { last_result: "cancelled".into(), last_error: None, orders_synced: n });
        Some(true)
    }

    /// Scheduler consistency sweep: running with no pending wait and no child.
    pub fn sweep_stuck_run_flag(&self) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.running && !g.pending && g.active_child.is_none() {
            Self::clear_stuck(&mut g);
            drop(g);
            log!("Cleared a stuck run flag (consistency sweep)");
            return true;
        }
        false
    }

    /// Shutdown hygiene: kill the active run's browser tree, synchronously.
    pub fn kill_active_run_group(&self) {
        let pid = self.inner.lock().unwrap().active_child;
        let Some(pid) = pid else { return };
        #[cfg(windows)]
        {
            let mut k = Command::new("taskkill");
            k.args(["/pid", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            crate::winproc::no_window(&mut k);
            let _ = k.status();
        }
        #[cfg(unix)]
        unsafe {
            if libc::kill(-(pid as i32), libc::SIGTERM) != 0 {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }

    /// The visible login window on the same profile. Detached, never killed.
    pub fn launch_setup_browser(self: &Arc<Self>) -> Result<(), String> {
        if self.running() {
            return Err("sync already in progress".into());
        }
        let Some(binary) = self.resolve_browser() else { return Err("no-browser".into()) };
        std::fs::create_dir_all(&self.profile_dir).map_err(|e| e.to_string())?;
        let ext = extension_dir();
        let mut cmd = Command::new(&binary);
        cmd.args(["--no-first-run", "--no-default-browser-check"])
            .arg(format!("--user-data-dir={}", self.profile_dir.display()))
            .arg(format!("--disable-extensions-except={}", ext.display()))
            .arg(format!("--load-extension={}", ext.display()))
            .arg("https://www.amazon.com/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detached(&mut cmd);
        let mut g = self.inner.lock().unwrap();
        match cmd.spawn() {
            Ok(mut child) => {
                g.setup_seq += 1;
                let seq = g.setup_seq;
                g.setup_child = Some(seq);
                let me = Arc::clone(self);
                std::thread::spawn(move || {
                    let _ = child.wait();
                    let mut g = me.inner.lock().unwrap();
                    if g.setup_child == Some(seq) {
                        g.setup_child = None;
                    }
                });
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    browser_cache().lock().unwrap().remove(&self.browser_path);
                }
            }
        }
        g.state.last_result = None;
        g.state.last_error = None;
        self.save_state_locked(&g.state);
        Ok(())
    }

    pub fn login_skip_mark(&self, today: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.login_skip_logged_day.as_deref() == Some(today) {
            return false;
        }
        g.login_skip_logged_day = Some(today.to_string());
        true
    }

    /// /api/sync-history/clear
    pub fn clear_history(&self) {
        let mut g = self.inner.lock().unwrap();
        g.state.history.clear();
        if !is_login_blocked(&g.state) {
            g.state.last_result = None;
            g.state.last_error = None;
        }
        g.state.orders_synced = 0;
        self.save_state_locked(&g.state);
    }

    pub fn prune_vote_timestamps(&self, present: &std::collections::HashSet<String>) {
        let mut g = self.inner.lock().unwrap();
        let before = g.state.vote_timestamps.len();
        g.state.vote_timestamps.retain(|(a, _)| present.contains(a));
        if g.state.vote_timestamps.len() != before {
            self.save_state_locked(&g.state);
        }
    }
}

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        libc::SIGTERM => "SIGTERM",
        libc::SIGKILL => "SIGKILL",
        libc::SIGINT => "SIGINT",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGABRT => "SIGABRT",
        libc::SIGHUP => "SIGHUP",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGBUS => "SIGBUS",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGUSR2 => "SIGUSR2",
        _ => "null",
    }
}

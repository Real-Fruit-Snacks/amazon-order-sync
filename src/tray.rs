//! The Windows tray app: one icon with the menu the C# tray had, the server
//! running in this same process, and the server's notifications shown as
//! Windows notifications. If something else already answers on the port
//! (an older install), the tray drives that server over HTTP instead.

use crate::config;
use crate::daemon::{self, Running};
use crate::js;
use crate::logbuf;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AmazonOrderSync";
const REVIEWS_URL: &str = "https://www.amazon.com/review/review-your-purchases/";
const APP_ICON: &[u8] = include_bytes!("../tray/app.ico");

const WM_TRAY: u32 = WM_APP + 1;
const WM_UI: u32 = WM_APP + 2;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const TIMER_POLL: usize = 1;

const ID_REVIEWS: usize = 10;
const ID_DASHBOARD: usize = 11;
const ID_SYNC: usize = 12;
const ID_SETUP: usize = 13;
const ID_NOTES: usize = 20;
const ID_CONFIG: usize = 21;
const ID_LOG: usize = 22;
const ID_EXTENSION: usize = 23;
const ID_STARTUP: usize = 30;
const ID_RESTART: usize = 31;
const ID_QUIT: usize = 32;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_w(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

// ---------------------------------------------------------------------------
// Paths, settings and logs (mirror the C# tray's)
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    config::default_data_dir()
}

fn tray_log_path() -> PathBuf {
    data_dir().join("tray.log")
}

pub fn server_log_path() -> PathBuf {
    data_dir().join("server.log")
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

static LOG_GATE: Mutex<()> = Mutex::new(());

fn tlog(line: &str) {
    let _g = LOG_GATE.lock().unwrap();
    let _ = std::fs::create_dir_all(data_dir());
    let p = tray_log_path();
    if std::fs::metadata(&p).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = write!(f, "{} {}\r\n", stamp(), line);
    }
}

struct Settings {
    port: u16,
    notes_path: PathBuf,
}

fn default_notes() -> PathBuf {
    config::home_dir().join("AmazonOrders")
}

fn load_settings() -> Settings {
    let path = config::config_path_from_env();
    let mut s = Settings { port: config::DEFAULT_PORT, notes_path: default_notes() };
    let Ok(bytes) = std::fs::read(&path) else { return s };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match js::json_parse(text) {
        Some(v) => {
            if let Some(p) = js::json_is_int(v.get("port")).filter(|p| *p > 0 && *p < 65536) {
                s.port = p as u16;
            }
            if let Some(Value::String(n)) = v.get("notesPath") {
                if !n.trim().is_empty() {
                    s.notes_path = config::expand_home(n);
                }
            }
        }
        None => tlog("config.json unreadable, using defaults"),
    }
    s
}

/// First run: write a default config so the server and the tray agree on
/// the notes folder. Never overwrites a file that exists.
fn ensure_default_config(notes: &Path) {
    let path = config::config_path_from_env();
    if path.exists() {
        return;
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let v = json!({
        "notesPath": notes.to_string_lossy(),
        "geminiApiKey": "",
        "port": 27483,
        "autoSyncEnabled": false,
        "autoSyncIntervalHours": 24,
        "notificationsEnabled": true
    });
    if std::fs::write(&path, js::stringify_pretty(&v) + "\r\n").is_ok() {
        tlog(&format!("Wrote default config to {}", path.display()));
    }
}

// ---------------------------------------------------------------------------
// The server: in-process, or an adopted one on the same port
// ---------------------------------------------------------------------------

struct Health {
    version: String,
    pid: i64,
    browser_found: bool,
    headless_running: bool,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).build()
}

fn health(port: u16) -> Option<Health> {
    let r = agent(Duration::from_secs(2)).get(&format!("http://127.0.0.1:{}/api/health", port)).call().ok()?;
    let v = js::json_parse(&r.into_string().ok()?)?;
    if v.get("ok") != Some(&Value::Bool(true)) {
        return None;
    }
    Some(Health {
        version: v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        pid: v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0),
        browser_found: v.get("browserFound") == Some(&Value::Bool(true)),
        headless_running: v.get("headlessRunning") == Some(&Value::Bool(true)),
    })
}

fn post(port: u16, path: &str, long: bool) -> (u16, String) {
    let a = agent(if long { Duration::from_secs(30 * 60) } else { Duration::from_secs(4) });
    match a.post(&format!("http://127.0.0.1:{}{}", port, path)).set("Content-Type", "application/json").send_string("{}") {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
        Err(e) => (0, e.to_string()),
    }
}

fn error_from_body(body: &str, fallback: &str) -> String {
    if let Some(Value::String(e)) = js::json_parse(body).and_then(|v| v.get("error").cloned()) {
        return e;
    }
    if body.trim().is_empty() {
        fallback.to_string()
    } else {
        body.to_string()
    }
}

/// Server output goes to server.log, as the C# tray's child output did.
fn install_server_log() {
    let p = server_log_path();
    let _ = std::fs::create_dir_all(data_dir());
    if std::fs::metadata(&p).map(|m| m.len() > 5 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&p).ok();
    let file = Mutex::new(file);
    if let Some(f) = file.lock().unwrap().as_mut() {
        let _ = write!(
            f,
            "==== {} starting: {} v{}\r\n",
            stamp(),
            std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            crate::http::VERSION
        );
    }
    logbuf::set_sink(Some(Box::new(move |line: &str| {
        if let Some(f) = file.lock().unwrap().as_mut() {
            let _ = write!(f, "{}\r\n", line);
        }
    })));
}

// ---------------------------------------------------------------------------
// UI state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Info,
    Warning,
    Error,
}

enum UiMsg {
    Status(String, String),
    SyncEnabled(bool),
    Toast(String, String, Kind),
    Started { ok: bool, initial: bool, message: String },
    Restart,
}

struct Tray {
    hwnd: HWND,
    settings: Settings,
    running: Option<Arc<Running>>,
    adopted: bool,
    status: String,
    sync_enabled: bool,
    busy: bool,
    quitting: bool,
    down_ticks: u32,
    notify_cursor: i64,
    icon: HICON,
}

// The window and icon handles are only used through Shell_NotifyIconW, which
// takes them from any thread.
unsafe impl Send for Tray {}

static TRAY: Mutex<Option<Tray>> = Mutex::new(None);
static INBOX: Mutex<Vec<UiMsg>> = Mutex::new(Vec::new());
static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
static TICKING: AtomicBool = AtomicBool::new(false);
static STARTING: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn send_ui(m: UiMsg) {
    INBOX.lock().unwrap().push(m);
    let h = HWND_MAIN.load(Ordering::SeqCst) as HWND;
    if !h.is_null() {
        unsafe {
            PostMessageW(h, WM_UI, 0, 0);
        }
    }
}

fn toast(title: &str, body: &str, kind: Kind) {
    send_ui(UiMsg::Toast(title.into(), body.into(), kind));
}

fn with_tray<R>(f: impl FnOnce(&mut Tray) -> R) -> Option<R> {
    TRAY.lock().unwrap().as_mut().map(f)
}

fn port() -> u16 {
    with_tray(|t| t.settings.port).unwrap_or(config::DEFAULT_PORT)
}

fn nid(t: &Tray) -> NOTIFYICONDATAW {
    let mut n: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = t.hwnd;
    n.uID = 1;
    n
}

fn add_icon(t: &Tray, tip: &str) {
    let mut n = nid(t);
    n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = t.icon;
    copy_w(&mut n.szTip, tip);
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &n);
        n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &n);
    }
}

fn set_status(t: &mut Tray, menu_text: &str, tooltip: &str) {
    t.status = menu_text.to_string();
    let tip: String = tooltip.chars().take(63).collect();
    let mut n = nid(t);
    n.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_w(&mut n.szTip, &tip);
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &n);
    }
}

fn show_toast(t: &Tray, title: &str, body: &str, kind: Kind) {
    if t.quitting {
        return;
    }
    let mut n = nid(t);
    n.uFlags = NIF_INFO;
    copy_w(&mut n.szInfoTitle, title);
    copy_w(&mut n.szInfo, if body.trim().is_empty() { " " } else { body });
    n.dwInfoFlags = match kind {
        Kind::Info => NIIF_INFO,
        Kind::Warning => NIIF_WARNING,
        Kind::Error => NIIF_ERROR,
    };
    unsafe {
        if Shell_NotifyIconW(NIM_MODIFY, &n) == 0 {
            tlog("Toast failed");
        }
    }
}

fn load_icon() -> HICON {
    // An .ico file: pick the entry closest to the small-icon size.
    let want = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    let d = APP_ICON;
    if d.len() >= 6 {
        let count = u16::from_le_bytes([d[4], d[5]]) as usize;
        let mut best: Option<(u32, usize, usize)> = None;
        for i in 0..count {
            let e = 6 + i * 16;
            if e + 16 > d.len() {
                break;
            }
            let wdt = if d[e] == 0 { 256 } else { d[e] as u32 };
            let size = u32::from_le_bytes([d[e + 8], d[e + 9], d[e + 10], d[e + 11]]) as usize;
            let off = u32::from_le_bytes([d[e + 12], d[e + 13], d[e + 14], d[e + 15]]) as usize;
            let score = if wdt >= want { wdt - want } else { 1000 + want - wdt };
            if off + size <= d.len() && best.map(|b| score < b.0).unwrap_or(true) {
                best = Some((score, off, size));
            }
        }
        if let Some((_, off, size)) = best {
            let h = unsafe {
                CreateIconFromResourceEx(
                    d[off..off + size].as_ptr(),
                    size as u32,
                    1,
                    0x0003_0000,
                    want as i32,
                    want as i32,
                    LR_DEFAULTCOLOR,
                )
            };
            if !h.is_null() {
                return h;
            }
        }
    }
    unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn shell_open(target: &str, args: Option<&str>) -> bool {
    let op = w("open");
    let t = w(target);
    let a = args.map(w);
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            t.as_ptr(),
            a.as_ref().map(|v| v.as_ptr()).unwrap_or(std::ptr::null()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    r as isize > 32
}

fn message_box(text: &str, flags: MESSAGEBOX_STYLE) {
    let t = w(text);
    let c = w("Amazon Order Sync");
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), flags);
    }
}

fn safe(what: &str, ok: bool) {
    if !ok {
        tlog(&format!("Action failed: {}", what));
        message_box(&format!("Could not {}.", what), MB_OK | MB_ICONERROR);
    }
}

fn open_reviews() {
    safe("open the browser", shell_open(REVIEWS_URL, None));
}

fn open_dashboard() {
    safe("open the browser", shell_open(&format!("http://127.0.0.1:{}/", port()), None));
}

fn open_folder(p: &Path) {
    let _ = std::fs::create_dir_all(p);
    safe("open the folder", shell_open("explorer.exe", Some(&format!("\"{}\"", p.display()))));
}

fn open_file(p: &Path) {
    if !p.exists() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, "");
    }
    safe("open the file", shell_open("notepad.exe", Some(&format!("\"{}\"", p.display()))));
}

fn exe_path() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
}

fn startup_enabled() -> bool {
    let key = w(RUN_KEY);
    let val = w(RUN_VALUE);
    let mut buf = vec![0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
    };
    if r != 0 {
        return false;
    }
    let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    !s.is_empty() && s.to_lowercase().contains(&exe_path().to_lowercase())
}

fn set_startup(enable: bool) {
    let key = w(RUN_KEY);
    let mut h: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut h,
            std::ptr::null_mut(),
        )
    };
    if r != 0 {
        toast("Could not change startup setting", &format!("Registry error {}", r), Kind::Error);
        return;
    }
    let val = w(RUN_VALUE);
    let res = unsafe {
        if enable {
            let data = w(&format!("\"{}\" --autostart", exe_path()));
            RegSetValueExW(h, val.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
        } else {
            let r = RegDeleteValueW(h, val.as_ptr());
            if r == 2 {
                0
            } else {
                r
            }
        }
    };
    unsafe {
        RegCloseKey(h);
    }
    if res != 0 {
        toast("Could not change startup setting", &format!("Registry error {}", res), Kind::Error);
    } else {
        tlog(&format!("Start with Windows: {}", if enable { "enabled" } else { "disabled" }));
    }
}

/// browser-extension\ next to the exe (the release zip's layout).
fn extension_folder() -> PathBuf {
    crate::headless::extension_dir()
}

fn sync_now() {
    let p = port();
    std::thread::spawn(move || {
        let Some(h) = health(p) else {
            toast("Server not running", "Use Restart server.", Kind::Warning);
            return;
        };
        if !h.browser_found {
            toast(
                "No browser for headless sync",
                "Install Brave/Chrome/Edge or set browserPath in config.json. You can still sync from the extension popup.",
                Kind::Warning,
            );
            return;
        }
        toast("Sync started", "Headless sync is running in the background.", Kind::Info);
        let (status, body) = post(p, "/api/sync-now", true);
        if status == 409 {
            toast("Sync already running", &error_from_body(&body, "Another sync or the setup window is active."), Kind::Info);
        } else if status != 200 && status != 0 {
            toast("Sync failed", &error_from_body(&body, &format!("HTTP {}", status)), Kind::Error);
        }
    });
}

fn setup_login() {
    let p = port();
    std::thread::spawn(move || {
        let (status, body) = post(p, "/api/headless-setup", false);
        if status == 200 {
            toast("Log in to Amazon", "A browser window opened. Sign in, then close it — auto-sync will reuse that login.", Kind::Info);
        } else {
            let fb = if status == 0 { "Server not reachable".to_string() } else { format!("HTTP {}", status) };
            toast("Could not open setup window", &error_from_body(&body, &fb), Kind::Error);
        }
    });
}

// ---------------------------------------------------------------------------
// Server lifecycle
// ---------------------------------------------------------------------------

fn start_server(initial: bool) {
    if STARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    with_tray(|t| set_status(t, "Starting server…", "Amazon Order Sync — starting"));
    std::thread::spawn(move || {
        let notes = with_tray(|t| t.settings.notes_path.clone()).unwrap_or_else(default_notes);
        ensure_default_config(&notes);
        let settings = load_settings();
        let p = settings.port;
        with_tray(|t| t.settings = settings);
        let (ok, message) = if let Some(h) = health(p) {
            with_tray(|t| {
                t.adopted = true;
                t.running = None;
            });
            tlog(&format!("Adopted an already-running server on port {} (v{} pid {})", p, h.version, h.pid));
            (true, String::new())
        } else {
            let on_shutdown: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {
                // POST /api/shutdown on the in-process server: stop serving.
                let r = with_tray(|t| t.running.take()).flatten();
                if let Some(r) = r {
                    r.stop();
                }
                send_ui(UiMsg::Status("Server stopped".into(), "Amazon Order Sync — server stopped".into()));
            });
            match daemon::start(on_shutdown) {
                Ok(r) => {
                    with_tray(|t| {
                        t.adopted = false;
                        t.running = Some(r);
                    });
                    tlog(&format!("server started v{} pid {}", crate::http::VERSION, std::process::id()));
                    (true, String::new())
                }
                Err(e) => {
                    let m = e.to_string();
                    tlog(&m);
                    (false, m)
                }
            }
        };
        STARTING.store(false, Ordering::SeqCst);
        send_ui(UiMsg::Started { ok, initial, message });
    });
}

fn stop_server_blocking() {
    let (running, adopted, p) = with_tray(|t| (t.running.take(), t.adopted, t.settings.port)).unwrap_or((None, false, 0));
    if let Some(r) = running {
        r.stop();
    } else if adopted {
        let _ = post(p, "/api/shutdown", false);
        for _ in 0..50 {
            if health(p).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn restart() {
    if with_tray(|t| std::mem::replace(&mut t.busy, true)).unwrap_or(true) {
        return;
    }
    with_tray(|t| set_status(t, "Restarting…", "Amazon Order Sync — restarting"));
    std::thread::spawn(|| {
        while STARTING.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        stop_server_blocking();
        with_tray(|t| t.busy = false);
        send_ui(UiMsg::Restart);
    });
}

fn tick() {
    if TICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (p, cursor) = with_tray(|t| (t.settings.port, t.notify_cursor)).unwrap_or((config::DEFAULT_PORT, -1));
    std::thread::spawn(move || {
        match health(p) {
            None => {
                let restart_now = with_tray(|t| {
                    t.down_ticks += 1;
                    t.adopted && t.down_ticks >= 3 && !t.busy
                })
                .unwrap_or(false);
                send_ui(UiMsg::Status("Server not responding".into(), "Amazon Order Sync — not responding".into()));
                if restart_now {
                    // The adopted server went away: take over.
                    with_tray(|t| t.adopted = false);
                    send_ui(UiMsg::Restart);
                }
            }
            Some(h) => {
                with_tray(|t| t.down_ticks = 0);
                let state = if h.headless_running { "syncing…" } else { "idle" };
                send_ui(UiMsg::Status(
                    format!("Running v{} · {}{}", h.version, state, if h.browser_found { "" } else { " · no browser" }),
                    format!("Amazon Order Sync — {}", state),
                ));
                send_ui(UiMsg::SyncEnabled(!h.headless_running));
                let url = if cursor < 0 {
                    format!("http://127.0.0.1:{}/api/notifications", p)
                } else {
                    format!("http://127.0.0.1:{}/api/notifications?after={}", p, cursor)
                };
                if let Ok(r) = agent(Duration::from_secs(4)).get(&url).call() {
                    if let Some(v) = r.into_string().ok().and_then(|t| js::json_parse(&t)) {
                        if cursor >= 0 {
                            for n in v.get("items").and_then(|i| i.as_array()).into_iter().flatten() {
                                let title = n.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let body = n.get("body").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let l = title.to_lowercase();
                                let kind = if l.contains("fail") || l.contains("missing") { Kind::Warning } else { Kind::Info };
                                toast(&title, &body, kind);
                            }
                        }
                        if let Some(next) = v.get("next").and_then(|n| n.as_i64()) {
                            with_tray(|t| t.notify_cursor = next);
                        }
                    }
                }
            }
        }
        TICKING.store(false, Ordering::SeqCst);
    });
}

fn quit(hwnd: HWND) {
    if with_tray(|t| std::mem::replace(&mut t.quitting, true)).unwrap_or(true) {
        return;
    }
    unsafe {
        KillTimer(hwnd, TIMER_POLL);
    }
    with_tray(|t| set_status(t, "Stopping…", "Amazon Order Sync — stopping"));
    let h = hwnd as isize;
    std::thread::spawn(move || {
        // "Quit (stops the server)": an adopted server is stopped too.
        stop_server_blocking();
        unsafe {
            PostMessageW(h as HWND, WM_CLOSE, 0, 0);
        }
    });
}

// ---------------------------------------------------------------------------
// Menu and window procedure
// ---------------------------------------------------------------------------

fn show_menu(hwnd: HWND) {
    let (status, sync_enabled) = with_tray(|t| (t.status.clone(), t.sync_enabled)).unwrap_or_default();
    unsafe {
        let m = CreatePopupMenu();
        let add = |id: usize, text: &str, flags: u32| {
            let t = w(text);
            AppendMenuW(m, MF_STRING | flags, id, t.as_ptr());
        };
        let sep = || {
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        };
        add(1, &format!("Amazon Order Sync v{}", crate::http::VERSION), MF_GRAYED);
        add(2, &status, MF_GRAYED);
        sep();
        add(ID_REVIEWS, "Review your purchases on Amazon", 0);
        add(ID_DASHBOARD, "Open dashboard", 0);
        add(ID_SYNC, "Sync now", if sync_enabled { 0 } else { MF_GRAYED });
        add(ID_SETUP, "Set up Amazon login…", 0);
        sep();
        add(ID_NOTES, "Open notes folder", 0);
        add(ID_CONFIG, "Edit config.json", 0);
        add(ID_LOG, "View server log", 0);
        add(ID_EXTENSION, "Open browser-extension folder (load in Brave)", 0);
        sep();
        add(ID_STARTUP, "Start with Windows", if startup_enabled() { MF_CHECKED } else { 0 });
        add(ID_RESTART, "Restart server", 0);
        add(ID_QUIT, "Quit (stops the server)", 0);
        SetMenuDefaultItem(m, ID_REVIEWS as u32, 0);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, std::ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
        if cmd != 0 {
            command(hwnd, cmd as usize);
        }
    }
}

fn command(hwnd: HWND, id: usize) {
    match id {
        ID_REVIEWS => open_reviews(),
        ID_DASHBOARD => open_dashboard(),
        ID_SYNC => sync_now(),
        ID_SETUP => setup_login(),
        ID_NOTES => open_folder(&with_tray(|t| t.settings.notes_path.clone()).unwrap_or_else(default_notes)),
        ID_CONFIG => open_file(&config::config_path_from_env()),
        ID_LOG => open_file(&server_log_path()),
        ID_EXTENSION => open_folder(&extension_folder()),
        ID_STARTUP => set_startup(!startup_enabled()),
        ID_RESTART => restart(),
        ID_QUIT => quit(hwnd),
        _ => {}
    }
}

fn drain_inbox() {
    let msgs: Vec<UiMsg> = std::mem::take(&mut *INBOX.lock().unwrap());
    for m in msgs {
        match m {
            UiMsg::Status(a, b) => {
                with_tray(|t| set_status(t, &a, &b));
            }
            UiMsg::SyncEnabled(e) => {
                with_tray(|t| t.sync_enabled = e);
            }
            UiMsg::Toast(a, b, k) => {
                with_tray(|t| show_toast(t, &a, &b, k));
            }
            UiMsg::Restart => {
                let ok = with_tray(|t| !t.quitting && !t.busy).unwrap_or(false);
                if ok {
                    start_server(false);
                }
            }
            UiMsg::Started { ok, initial, message } => {
                let p = port();
                if ok {
                    with_tray(|t| t.down_ticks = 0);
                    let h = health(p);
                    if initial {
                        toast("Amazon Order Sync", &format!("Running — dashboard at http://127.0.0.1:{}/", p), Kind::Info);
                    }
                    if let Some(h) = h {
                        if !h.browser_found {
                            toast(
                                "No browser found for auto-sync",
                                "Install Brave/Chrome/Edge or set \"browserPath\" in config.json.",
                                Kind::Warning,
                            );
                        }
                    }
                    tick();
                } else {
                    with_tray(|t| set_status(t, "Server failed to start", "Amazon Order Sync — server not running"));
                    toast("Amazon Order Sync could not start", if message.is_empty() { "Unknown error" } else { &message }, Kind::Error);
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let tc = TASKBAR_CREATED.load(Ordering::SeqCst);
    if tc != 0 && msg == tc {
        // Explorer restarted: put the icon back.
        let status = with_tray(|t| t.status.clone()).unwrap_or_default();
        with_tray(|t| add_icon(t, &format!("Amazon Order Sync — {}", status)));
        return 0;
    }
    match msg {
        WM_TRAY => {
            let event = (lparam as u32) & 0xffff;
            match event {
                WM_CONTEXTMENU | WM_RBUTTONUP => show_menu(hwnd),
                WM_LBUTTONDBLCLK | NIN_BALLOONUSERCLICK => open_reviews(),
                NIN_SELECT | NIN_KEYSELECT => {}
                _ => {}
            }
            0
        }
        WM_UI => {
            drain_inbox();
            0
        }
        WM_TIMER => {
            if wparam == TIMER_POLL {
                let skip = with_tray(|t| t.quitting || t.busy).unwrap_or(true);
                if !skip && !STARTING.load(Ordering::SeqCst) {
                    tick();
                }
            }
            0
        }
        WM_CLOSE => {
            with_tray(|t| {
                let n = nid(t);
                Shell_NotifyIconW(NIM_DELETE, &n);
            });
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run(args: &[String]) -> i32 {
    // One tray per user session: a second launch opens the dashboard.
    let mname = w(r"Local\AmazonOrderSync.Tray.SingleInstance");
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, mname.as_ptr()) };
    if !mutex.is_null() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = shell_open(&format!("http://127.0.0.1:{}/", load_settings().port), None);
        return 0;
    }
    tlog(&format!("Tray starting v{} (args: {})", crate::http::VERSION, args.join(" ")));
    install_server_log();
    std::panic::set_hook(Box::new(|info| {
        tlog(&format!("Unhandled panic: {}", info));
    }));

    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = w("AmazonOrderSyncTray");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = w("Amazon Order Sync");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            tlog("CreateWindowExW failed");
            return 1;
        }
        HWND_MAIN.store(hwnd as isize, Ordering::SeqCst);
        let tc = w("TaskbarCreated");
        TASKBAR_CREATED.store(RegisterWindowMessageW(tc.as_ptr()), Ordering::SeqCst);

        let tray = Tray {
            hwnd,
            settings: load_settings(),
            running: None,
            adopted: false,
            status: "Starting…".into(),
            sync_enabled: true,
            busy: false,
            quitting: false,
            down_ticks: 0,
            notify_cursor: -1,
            icon: load_icon(),
        };
        add_icon(&tray, "Amazon Order Sync — starting");
        *TRAY.lock().unwrap() = Some(tray);
        SetTimer(hwnd, TIMER_POLL, 3000, None);
        start_server(true);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Belt and braces: never leave a browser or the lock behind.
    if let Some(r) = with_tray(|t| t.running.take()).flatten() {
        r.exit_hygiene();
    }
    tlog("Tray exited");
    0
}

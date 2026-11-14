//! Starting and stopping the server: config, the single-instance lock, the
//! listener, the 30-minute scheduler, and shutdown hygiene.

use crate::config::{self, Config};
use crate::headless::{self, Headless, RunResult};
use crate::http::{self, App};
use crate::js;
use crate::jsdate;
use crate::store::{self, Store};
use crate::{elog, log};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const SCHEDULER_TICK_MS: u64 = 30 * 60 * 1000;
const INSTANCE_LOCK_NAME: &str = ".daemon.lock";

pub fn skip_lock() -> bool {
    std::env::var("AOS_SKIP_INSTANCE_LOCK").ok().as_deref() == Some("1")
}

/// Ok, or the message for a live daemon on the same notes folder.
pub fn acquire_instance_lock(notes_path: &Path, port: u16) -> Result<(), String> {
    if skip_lock() {
        return Ok(());
    }
    let lock = notes_path.join(INSTANCE_LOCK_NAME);
    match std::fs::read(&lock) {
        Ok(bytes) => {
            if let Some(v) = js::json_parse(&String::from_utf8_lossy(&bytes)) {
                if let Some(pid) = js::json_is_int(v.get("pid")) {
                    if pid > 0 && pid != std::process::id() as i64 && headless::is_pid_alive(pid) {
                        return Err(format!(
                            "another daemon (pid {}) is already serving this notes folder ({}) — refusing to start. Stop it first, or set AOS_SKIP_INSTANCE_LOCK=1 to bypass.",
                            pid,
                            notes_path.display()
                        ));
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    std::fs::create_dir_all(notes_path).map_err(|e| e.to_string())?;
    let body =
        js::stringify_pretty(&json!({ "pid": std::process::id(), "port": port, "startedAt": jsdate::iso_string(jsdate::now_ms()) })) + "\n";
    std::fs::write(&lock, body).map_err(|e| e.to_string())
}

/// Remove the lock only if it is ours.
pub fn release_instance_lock(notes_path: &Path) {
    if skip_lock() {
        return;
    }
    let lock = notes_path.join(INSTANCE_LOCK_NAME);
    if let Ok(bytes) = std::fs::read(&lock) {
        if let Some(v) = js::json_parse(&String::from_utf8_lossy(&bytes)) {
            if js::json_is_int(v.get("pid")) == Some(std::process::id() as i64) {
                let _ = std::fs::remove_file(&lock);
            }
        }
    }
}

pub struct Running {
    pub app: Arc<App>,
    pub cfg: Config,
    server: Arc<tiny_http::Server>,
    stop_flag: Arc<(Mutex<bool>, Condvar)>,
    stopped: AtomicBool,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

pub enum StartError {
    Config(String),
    Locked(String),
    PortInUse(u16),
    Bind(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            StartError::Config(m) => write!(f, "Failed to start: {}", m),
            StartError::Locked(m) => write!(f, "{}", m),
            StartError::PortInUse(p) => write!(f, "Port {} is already in use. Is another amazon-order-sync server running?", p),
            StartError::Bind(m) => write!(f, "Server error: {}", m),
        }
    }
}

pub fn make_headless(cfg: &Config) -> Arc<Headless> {
    let h = Headless::new(
        cfg.notes_path.clone(),
        cfg.auto_sync_enabled,
        cfg.auto_sync_interval_hours,
        cfg.browser_path.clone(),
        cfg.notifications_enabled,
    );
    h.load_state();
    h
}

/// Start the server. `on_shutdown` runs when POST /api/shutdown arrives.
pub fn start(on_shutdown: Arc<dyn Fn() + Send + Sync>) -> Result<Arc<Running>, StartError> {
    let cfg = config::load().map_err(StartError::Config)?;
    let store = Arc::new(Store::new(&cfg));
    let headless = make_headless(&cfg);

    acquire_instance_lock(&cfg.notes_path, cfg.port).map_err(StartError::Locked)?;
    store.ensure_folder_exists("Orders");

    let server = match tiny_http::Server::http(("127.0.0.1", cfg.port)) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let in_use = e
                .downcast_ref::<std::io::Error>()
                .map(|io| io.kind() == std::io::ErrorKind::AddrInUse)
                .unwrap_or_else(|| e.to_string().to_lowercase().contains("in use") || e.to_string().contains("10048"));
            return Err(if in_use { StartError::PortInUse(cfg.port) } else { StartError::Bind(e.to_string()) });
        }
    };
    let bound = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(cfg.port);
    let app = Arc::new(App {
        store: Arc::clone(&store),
        headless: Arc::clone(&headless),
        port: cfg.port,
        bound_port: std::sync::atomic::AtomicU16::new(bound),
        on_shutdown,
    });

    store::notify(&format!("Amazon Sync Server running on port {}", cfg.port));
    log!("Dashboard:  http://127.0.0.1:{}/", cfg.port);
    log!("Notes path: {}", cfg.notes_path.display());
    log!("Config:     {}", cfg.config_path.display());
    if cfg.gemini_api_key.is_empty() {
        log!("No Gemini API key configured; AI review generation is disabled.");
    }

    let running = Arc::new(Running {
        app: Arc::clone(&app),
        cfg: cfg.clone(),
        server: Arc::clone(&server),
        stop_flag: Arc::new((Mutex::new(false), Condvar::new())),
        stopped: AtomicBool::new(false),
        threads: Mutex::new(Vec::new()),
    });

    let (a, s) = (Arc::clone(&app), Arc::clone(&server));
    let t1 = std::thread::Builder::new().name("http".into()).spawn(move || http::serve(a, s)).expect("http thread");
    let flag = Arc::clone(&running.stop_flag);
    let a = Arc::clone(&app);
    let interval = cfg.review_check_interval_days;
    let tick_ms = std::env::var("AOS_SCHEDULER_TICK_MS").ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(SCHEDULER_TICK_MS);
    let t2 = std::thread::Builder::new()
        .name("scheduler".into())
        .spawn(move || loop {
            {
                let (m, c) = &*flag;
                let g = m.lock().unwrap();
                let (g, _) = c.wait_timeout_while(g, Duration::from_millis(tick_ms), |stop| !*stop).unwrap();
                if *g {
                    return;
                }
            }
            scheduler_tick(&a, interval);
        })
        .expect("scheduler thread");
    running.threads.lock().unwrap().extend([t1, t2]);
    Ok(running)
}

fn scheduler_tick(app: &Arc<App>, review_interval_days: i64) {
    let h = &app.headless;
    let now = jsdate::now_ms() as f64;
    let browser_found = h.resolve_browser().is_some();
    let login_blocked = h.login_blocked();
    let st = h.state();
    let running = h.running();
    let interval_ms = h.interval_hours as f64 * 3_600_000.0;
    let review_ms = review_interval_days as f64 * 86_400_000.0;
    if headless::is_due(h.enabled, browser_found, running, login_blocked, st.last_run_at, interval_ms, now) {
        let a = Arc::clone(app);
        std::thread::spawn(move || {
            let _ = a.headless.run_headless_sync(None);
        });
    } else if headless::is_due(h.enabled, browser_found, running, login_blocked, st.last_review_check_at, review_ms, now) {
        let a = Arc::clone(app);
        std::thread::spawn(move || {
            let q = http::compute_review_check_queue(&a.store, &a.headless, review_interval_days, jsdate::now_ms() as f64);
            if let Some(first) = q.first() {
                if let RunResult::Already = a.headless.run_review_check(first) {}
            }
        });
    } else if login_blocked {
        let would = headless::is_due(h.enabled, browser_found, running, false, st.last_run_at, interval_ms, now)
            || headless::is_due(h.enabled, browser_found, running, false, st.last_review_check_at, review_ms, now);
        let today = jsdate::local_date_key(jsdate::now_ms());
        if would && h.login_skip_mark(&today) {
            log!("Scheduler skip: login-blocked — run SETUP LOGIN to re-authenticate.");
        }
    }
    http::prune_vote_timestamps(&app.store, &app.headless);
    h.sweep_stuck_run_flag();
}

impl Running {
    /// Kill the active run's browser, release the lock, stop listening.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.app.headless.kill_active_run_group();
        release_instance_lock(&self.cfg.notes_path);
        {
            let (m, c) = &*self.stop_flag;
            *m.lock().unwrap() = true;
            c.notify_all();
        }
        self.server.unblock();
        for t in self.threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
    }

    /// The process is about to exit: kill the browser and drop the lock.
    pub fn exit_hygiene(&self) {
        self.app.headless.kill_active_run_group();
        release_instance_lock(&self.cfg.notes_path);
    }
}

/// `--setup-headless`: open the visible login window and exit.
pub fn setup_headless_cli() -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            elog!("Failed to start: {}", e);
            return 1;
        }
    };
    let h = make_headless(&cfg);
    if h.launch_setup_browser().is_err() {
        elog!("No browser found. Install Brave/Chrome/Edge/Chromium (or set \"browserPath\" in the config) and try again.");
        return 1;
    }
    log!("Setup browser opened on https://www.amazon.com/");
    log!("1. Log into your Amazon account in that window.");
    log!("2. Close the window when done — the login is stored in the");
    log!("   dedicated profile at {}", h.profile_dir.display());
    log!("Headless auto-sync runs will reuse that login.");
    0
}

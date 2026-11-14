// On Windows the program is a tray app: no console window when it starts
// from Explorer or the Run key. `serve` attaches to the parent's console.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod backfill;
mod config;
mod daemon;
mod gemini;
mod headless;
mod http;
mod js;
mod jsdate;
mod logbuf;
mod notes;
mod store;
#[cfg(windows)]
mod tray;
#[cfg(test)]
mod unit_tests;
#[cfg(windows)]
mod winproc;

use std::sync::{Arc, Mutex};

const HELP: &str = "amazon-order-sync — syncs your Amazon orders into local Markdown notes

Usage:
  amazon-order-sync                 Windows: the tray app (starts the server). Elsewhere: same as serve.
  amazon-order-sync serve           Run the server in the foreground on 127.0.0.1.
  amazon-order-sync --setup-headless
                                    Open the browser window to log in to Amazon for headless sync.
  amazon-order-sync config          Print the config path, notes folder and port in use.
  amazon-order-sync --version       Print the version.

Config: AMAZON_SYNC_CONFIG, or the default config.json location
(%APPDATA%\\amazon-order-sync on Windows, $XDG_CONFIG_HOME/amazon-order-sync elsewhere).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);

    if has("--version") || has("-V") {
        attach_console();
        println!("{}", http::VERSION);
        return;
    }
    if has("--help") || has("-h") {
        attach_console();
        println!("{}", HELP);
        return;
    }
    if has("--setup-headless") || args.first().map(String::as_str) == Some("setup") {
        attach_console();
        std::process::exit(daemon::setup_headless_cli());
    }
    if args.first().map(String::as_str) == Some("config") {
        // For install scripts: where the config is and what it resolves to.
        attach_console();
        match config::load() {
            Ok(c) => {
                println!("configPath={}", c.config_path.display());
                println!("notesPath={}", c.notes_path.display());
                println!("port={}", c.port);
                return;
            }
            Err(e) => {
                eprintln!("Failed to read the config: {}", e);
                std::process::exit(1);
            }
        }
    }
    let serve = args.first().map(String::as_str) == Some("serve") || has("--serve");
    #[cfg(windows)]
    if !serve {
        let code = tray::run(&args);
        std::process::exit(code);
    }
    let _ = serve;
    attach_console();
    std::process::exit(serve_foreground());
}

/// A GUI-subsystem program has no console: borrow the parent's, if any, so
/// `serve` and `--version` print in a terminal. Redirected handles (a pipe
/// from a test or the tray) are kept as they are.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(unix)]
fn block_signals() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

pub fn serve_foreground() -> i32 {
    // Signals are blocked before any thread starts, so only the waiter
    // below receives SIGTERM/SIGINT.
    #[cfg(unix)]
    let sigset = block_signals();

    let holder: Arc<Mutex<Option<Arc<daemon::Running>>>> = Arc::new(Mutex::new(None));
    let h2 = Arc::clone(&holder);
    let on_shutdown: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        if let Some(r) = h2.lock().unwrap().as_ref() {
            r.exit_hygiene();
        }
        std::process::exit(0);
    });
    let running = match daemon::start(on_shutdown) {
        Ok(r) => r,
        Err(e) => {
            elog!("{}", e);
            if let daemon::StartError::PortInUse(_) | daemon::StartError::Bind(_) = e {
                store::notify("Failed to start Amazon Sync Server. Port might be in use.");
            }
            return 1;
        }
    };
    *holder.lock().unwrap() = Some(Arc::clone(&running));

    #[cfg(unix)]
    {
        let mut sig: libc::c_int = 0;
        unsafe {
            libc::sigwait(&sigset, &mut sig);
        }
        running.exit_hygiene();
        0
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        static RUNNING: std::sync::OnceLock<Arc<daemon::Running>> = std::sync::OnceLock::new();
        let _ = RUNNING.set(Arc::clone(&running));
        unsafe extern "system" fn handler(_ctrl: u32) -> i32 {
            if let Some(r) = RUNNING.get() {
                r.exit_hygiene();
            }
            std::process::exit(0);
        }
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
        loop {
            std::thread::park();
        }
    }
}

//! A stand-in browser for the headless tests, on any OS: the server spawns
//! it as `browserPath`. AOS_FAKE_BROWSER picks what it does:
//!   sleep          sleep 60 s (the default)
//!   exit:N         exit with code N right away
//!   headless-only  sleep 60 s when given --headless=new, else exit 0
//! AOS_FAKE_BROWSER_ARGV, when set, is a file that gets one argument per line.

use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(p) = std::env::var("AOS_FAKE_BROWSER_ARGV") {
        if !p.is_empty() {
            let _ = std::fs::write(&p, args.iter().map(|a| format!("{}\n", a)).collect::<String>());
        }
    }
    let mode = std::env::var("AOS_FAKE_BROWSER").unwrap_or_else(|_| "sleep".into());
    if let Some(code) = mode.strip_prefix("exit:") {
        std::process::exit(code.parse().unwrap_or(1));
    }
    if mode == "headless-only" && !args.iter().any(|a| a == "--headless=new") {
        std::process::exit(0);
    }
    std::thread::sleep(Duration::from_secs(60));
}

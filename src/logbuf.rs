//! The daemon log ring buffer (GET /api/logs) and the notification ring
//! buffer (GET /api/notifications). Every log line also goes to stdout or
//! stderr, as console.log/console.error did.

use crate::jsdate::now_ms;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;

const LOG_BUFFER_CAP: usize = 500;
const NOTIFICATION_BUFFER_CAP: usize = 50;

struct Ring {
    items: VecDeque<Value>,
    index: u64,
}

static LOGS: Mutex<Ring> = Mutex::new(Ring { items: VecDeque::new(), index: 0 });
static NOTES: Mutex<Ring> = Mutex::new(Ring { items: VecDeque::new(), index: 0 });

/// Echo to the console and the tray's log file sink (when the tray runs the
/// server in-process it captures the output itself).
type Sink = Box<dyn Fn(&str) + Send>;
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

pub fn set_sink(f: Option<Sink>) {
    *SINK.lock().unwrap() = f;
}

pub fn push_log_line(text: String) {
    let mut r = LOGS.lock().unwrap();
    r.index += 1;
    let i = r.index;
    r.items.push_back(json!({ "i": i, "t": now_ms(), "text": text }));
    if r.items.len() > LOG_BUFFER_CAP {
        r.items.pop_front();
    }
}

fn emit(text: String, err: bool) {
    push_log_line(text.clone());
    if let Some(f) = SINK.lock().unwrap().as_ref() {
        f(&text);
        return;
    }
    if err {
        let mut e = std::io::stderr().lock();
        let _ = writeln!(e, "{}", text);
    } else {
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{}", text);
        let _ = o.flush();
    }
}

pub fn log_line(text: String) {
    emit(text, false);
}

pub fn err_line(text: String) {
    emit(text, true);
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::logbuf::log_line(format!($($arg)*)) };
}

#[macro_export]
macro_rules! elog {
    ($($arg:tt)*) => { $crate::logbuf::err_line(format!($($arg)*)) };
}

/// `after` (a previous `next`) yields only newer lines; None yields the last 100.
pub fn read_logs(after: Option<f64>) -> Value {
    let r = LOGS.lock().unwrap();
    let lines: Vec<Value> = match after {
        Some(a) => r.items.iter().filter(|l| l["i"].as_f64().unwrap_or(0.0) > a).cloned().collect(),
        None => {
            let skip = r.items.len().saturating_sub(100);
            r.items.iter().skip(skip).cloned().collect()
        }
    };
    json!({ "lines": lines, "next": r.index })
}

pub fn clear_logs() {
    LOGS.lock().unwrap().items.clear();
}

pub fn push_notification(title: &str, body: &str) {
    let mut r = NOTES.lock().unwrap();
    r.index += 1;
    let i = r.index;
    r.items.push_back(json!({ "i": i, "t": now_ms(), "title": title, "body": body }));
    if r.items.len() > NOTIFICATION_BUFFER_CAP {
        r.items.pop_front();
    }
}

/// No cursor returns no items (just `next`), so a new poller never replays.
pub fn read_notifications(after: Option<f64>) -> Value {
    let r = NOTES.lock().unwrap();
    let items: Vec<Value> = match after {
        Some(a) => r.items.iter().filter(|n| n["i"].as_f64().unwrap_or(0.0) > a).cloned().collect(),
        None => Vec::new(),
    };
    json!({ "items": items, "next": r.index })
}

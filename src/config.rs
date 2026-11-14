//! config.json: where it lives, the defaults, and the validation of each key.

use crate::js::{self, json_is_int};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 27483;
pub const DEFAULT_SYNC_REMINDER_DAYS: i64 = 7;
pub const DEFAULT_AUTOSYNC_INTERVAL_HOURS: i64 = 24;
pub const DEFAULT_REVIEW_CHECK_INTERVAL_DAYS: i64 = 7;
pub const DEFAULT_REVIEW_CUTOFF_DAYS: i64 = 60;
pub const DEFAULT_GEMINI_DAILY_LIMIT: i64 = 1000;
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-flash-lite";
pub const DEFAULT_GEMINI_TEMPERATURE: f64 = 0.9;

pub fn home_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    PathBuf::from(".")
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Lookup of an environment variable (non-empty values only).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn real_env(name: &str) -> Option<String> {
    env_nonempty(name)
}

pub fn default_config_path() -> PathBuf {
    default_config_path_with(cfg!(windows), &real_env, &home_dir())
}

/// %APPDATA% on Windows, $XDG_CONFIG_HOME elsewhere, each under the home
/// folder when unset.
pub fn default_config_path_with(windows: bool, env: Env, home: &Path) -> PathBuf {
    if windows {
        let base = env("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Roaming"));
        return base.join("amazon-order-sync").join("config.json");
    }
    let xdg = env("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    xdg.join("amazon-order-sync").join("config.json")
}

/// Per-user data dir: holds the headless browser profile (and, on Windows,
/// the tray's logs).
pub fn default_data_dir() -> PathBuf {
    default_data_dir_with(cfg!(windows), &real_env, &home_dir())
}

pub fn default_data_dir_with(windows: bool, env: Env, home: &Path) -> PathBuf {
    if windows {
        let base = env("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Local"));
        return base.join("amazon-order-sync");
    }
    let xdg = env("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local").join("share"));
    xdg.join("amazon-order-sync")
}

pub fn expand_home(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir();
    }
    if p.starts_with("~/") || p.starts_with("~\\") {
        return home_dir().join(&p[2..]);
    }
    PathBuf::from(p)
}

/// path.resolve(): absolute, with `.` and `..` folded.
pub fn resolve(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p) };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    // Node's path.resolve drops a trailing separator.
    out
}

#[derive(Clone, Debug)]
pub struct Config {
    pub config_path: PathBuf,
    pub notes_path: PathBuf,
    pub gemini_api_key: String,
    pub port: u16,
    pub sync_reminder_days: i64,
    pub auto_sync_enabled: bool,
    pub auto_sync_interval_hours: i64,
    pub browser_path: String,
    pub notifications_enabled: bool,
    pub review_check_interval_days: i64,
    pub gemini_daily_limit: i64,
    pub review_cutoff_days: i64,
    pub gemini_model: String,
    pub gemini_temperature: f64,
    pub style_exemplars: bool,
}

fn pos_int(v: Option<&Value>, default: i64) -> i64 {
    match json_is_int(v) {
        Some(n) if n > 0 => n,
        _ => default,
    }
}

pub fn config_path_from_env() -> PathBuf {
    env_nonempty("AMAZON_SYNC_CONFIG").map(PathBuf::from).unwrap_or_else(default_config_path)
}

pub fn load() -> Result<Config, String> {
    let config_path = config_path_from_env();
    let cfg: Value = match std::fs::read(&config_path) {
        Ok(bytes) => {
            let raw = String::from_utf8_lossy(&bytes).into_owned();
            // Tolerate a UTF-8 BOM (Windows editors and PowerShell add one).
            let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
            let v = js::json_parse(raw).ok_or_else(|| format!("Unexpected token in JSON at {}", config_path.display()))?;
            if !v.is_object() {
                return Err("config.json must contain a JSON object".into());
            }
            v
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let v = json!({ "notesPath": "~/AmazonOrders", "geminiApiKey": "", "port": DEFAULT_PORT });
            if let Some(dir) = config_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&config_path, js::stringify_pretty(&v) + "\n").map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
            }
            crate::log!("Created default config at {}", config_path.display());
            v
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok(from_value(config_path, &cfg))
}

pub fn from_value(config_path: PathBuf, c: &Value) -> Config {
    let notes_raw = match c.get("notesPath") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        // `config.notesPath || '~/AmazonOrders'`: any other truthy value is
        // String()-ed by path.resolve, which would throw; fall back instead.
        _ => "~/AmazonOrders".into(),
    };
    let notes_path = resolve(&expand_home(&notes_raw));
    let gemini_api_key = match c.get("geminiApiKey") {
        Some(Value::String(s)) if !js::trim(s).is_empty() => s.clone(),
        _ => std::env::var("GEMINI_API_KEY").unwrap_or_default(),
    };
    let port = match json_is_int(c.get("port")) {
        Some(p) if p > 0 && p <= 65535 => p as u16,
        _ => DEFAULT_PORT,
    };
    let review_cutoff_days = match json_is_int(c.get("reviewCutoffDays")) {
        Some(n) if n >= 0 => n,
        _ => DEFAULT_REVIEW_CUTOFF_DAYS,
    };
    let gemini_model = match c.get("geminiModel") {
        Some(Value::String(s)) if valid_model(js::trim(s)) => js::trim(s).to_string(),
        _ => DEFAULT_GEMINI_MODEL.into(),
    };
    let gemini_temperature = match c.get("geminiTemperature") {
        Some(Value::Number(n)) => match n.as_f64() {
            Some(t) if (0.0..=2.0).contains(&t) => t,
            _ => DEFAULT_GEMINI_TEMPERATURE,
        },
        _ => DEFAULT_GEMINI_TEMPERATURE,
    };
    Config {
        config_path,
        notes_path,
        gemini_api_key,
        port,
        sync_reminder_days: pos_int(c.get("syncReminderDays"), DEFAULT_SYNC_REMINDER_DAYS),
        auto_sync_enabled: c.get("autoSyncEnabled") == Some(&Value::Bool(true)),
        auto_sync_interval_hours: pos_int(c.get("autoSyncIntervalHours"), DEFAULT_AUTOSYNC_INTERVAL_HOURS),
        browser_path: match c.get("browserPath") {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        },
        notifications_enabled: c.get("notificationsEnabled") != Some(&Value::Bool(false)),
        review_check_interval_days: pos_int(c.get("reviewCheckIntervalDays"), DEFAULT_REVIEW_CHECK_INTERVAL_DAYS),
        gemini_daily_limit: pos_int(c.get("geminiDailyLimit"), DEFAULT_GEMINI_DAILY_LIMIT),
        review_cutoff_days,
        gemini_model,
        gemini_temperature,
        style_exemplars: c.get("styleExemplars") != Some(&Value::Bool(false)),
    }
}

/// /^[\w.-]+$/
pub fn valid_model(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

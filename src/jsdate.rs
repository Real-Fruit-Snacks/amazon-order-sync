//! `new Date(string)` as V8 parses it (the ES ISO format, then V8's legacy
//! parser for strings like "August 1, 2026"), plus the local-time getters the
//! server uses. Times are milliseconds since the epoch, as in JS.

use chrono::{Datelike, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};

const MS_PER_DAY: f64 = 86_400_000.0;

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm; m 1..12.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// MakeDay(year, month0, date) with month/day overflow like JS.
fn make_day(year: f64, month0: f64, date: f64) -> f64 {
    let ym = year + (month0 / 12.0).floor();
    let mn = month0.rem_euclid(12.0);
    let days = days_from_civil(ym as i64, mn as i64 + 1, 1) as f64;
    days + date - 1.0
}

fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    h * 3_600_000.0 + m * 60_000.0 + s * 1000.0 + ms
}

/// Local wall-clock milliseconds -> epoch milliseconds (JS UTC(t)).
fn local_to_utc(local_ms: f64) -> f64 {
    let secs = (local_ms / 1000.0).floor() as i64;
    let rem_ms = local_ms - (secs as f64) * 1000.0;
    let Some(naive) = chrono::DateTime::from_timestamp(secs, 0).map(|d| d.naive_utc()) else {
        return f64::NAN;
    };
    let offset_ms = |n: NaiveDateTime| -> Option<i64> {
        match Local.from_local_datetime(&n) {
            LocalResult::Single(dt) => Some(dt.offset().local_minus_utc() as i64 * 1000),
            LocalResult::Ambiguous(a, _) => Some(a.offset().local_minus_utc() as i64 * 1000),
            LocalResult::None => None,
        }
    };
    let off = offset_ms(naive)
        .or_else(|| {
            // In a DST gap: use the offset in effect before the transition.
            let before = naive - chrono::Duration::hours(3);
            offset_ms(before)
        })
        .unwrap_or(0);
    local_ms - rem_ms + rem_ms - off as f64
}

pub struct LocalParts {
    pub year: i32,
    pub month0: u32,
    pub day: u32,
}

pub fn local_parts(ms: f64) -> LocalParts {
    let dt = Local.timestamp_millis_opt(ms as i64).single().unwrap_or_else(|| Local.timestamp_millis_opt(0).unwrap());
    LocalParts { year: dt.year(), month0: dt.month0(), day: dt.day() }
}

/// localDateKey(): YYYY-MM-DD in local time.
pub fn local_date_key(ms: i64) -> String {
    let p = local_parts(ms as f64);
    format!("{}-{:02}-{:02}", p.year, p.month0 + 1, p.day)
}

pub fn same_local_day(a: f64, b: f64) -> bool {
    let x = local_parts(a);
    let y = local_parts(b);
    x.year == y.year && x.month0 == y.month0 && x.day == y.day
}

pub fn iso_string(ms: i64) -> String {
    let dt = Utc.timestamp_millis_opt(ms).single().unwrap_or_else(|| Utc.timestamp_millis_opt(0).unwrap());
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub const MONTH_NAMES: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn time_clip(t: f64) -> Option<f64> {
    if !t.is_finite() || t.abs() > 8.64e15 {
        None
    } else {
        Some(t.trunc() + 0.0)
    }
}

// ---------------------------------------------------------------------------
// ES ISO format: YYYY[-MM[-DD]][THH:mm[:ss[.sss]][Z|±HH:mm]]
// ---------------------------------------------------------------------------

fn parse_iso(s: &str) -> Option<Option<f64>> {
    let b = s.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, n: usize| -> Option<i64> {
        if *i + n > b.len() || !b[*i..*i + n].iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v = s[*i..*i + n].parse().ok()?;
        *i += n;
        Some(v)
    };
    let year = if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        let neg = b[i] == b'-';
        i += 1;
        let y = num(&mut i, 6)?;
        if neg && y == 0 {
            return Some(None);
        }
        if neg {
            -y
        } else {
            y
        }
    } else {
        num(&mut i, 4)?
    };
    let mut month = 1;
    let mut day = 1;
    if i < b.len() && b[i] == b'-' {
        i += 1;
        month = num(&mut i, 2)?;
        if i < b.len() && b[i] == b'-' {
            i += 1;
            day = num(&mut i, 2)?;
        }
    }
    if i == b.len() {
        if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
            return Some(None);
        }
        let t = make_day(year as f64, (month - 1) as f64, day as f64) * MS_PER_DAY;
        return Some(time_clip(t));
    }
    if b[i] != b'T' {
        return None;
    }
    i += 1;
    let h = num(&mut i, 2)?;
    if i >= b.len() || b[i] != b':' {
        return None;
    }
    i += 1;
    let mi = num(&mut i, 2)?;
    let mut sec = 0;
    let mut ms = 0f64;
    if i < b.len() && b[i] == b':' {
        i += 1;
        sec = num(&mut i, 2)?;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return None;
            }
            let frac = &s[start..i];
            let f: String = frac.chars().chain("000".chars()).take(3).collect();
            ms = f.parse::<f64>().ok()?;
        }
    }
    let mut offset: Option<i64> = None;
    if i < b.len() {
        match b[i] {
            b'Z' => {
                offset = Some(0);
                i += 1;
            }
            b'+' | b'-' => {
                let sign = if b[i] == b'-' { -1 } else { 1 };
                i += 1;
                let oh = num(&mut i, 2)?;
                if i >= b.len() || b[i] != b':' {
                    return None;
                }
                i += 1;
                let om = num(&mut i, 2)?;
                offset = Some(sign * (oh * 60 + om));
            }
            _ => return None,
        }
    }
    if i != b.len() {
        return None;
    }
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Some(None);
    }
    if h > 24 || mi > 59 || sec > 59 || (h == 24 && (mi > 0 || sec > 0 || ms > 0.0)) {
        return Some(None);
    }
    let local = make_day(year as f64, (month - 1) as f64, day as f64) * MS_PER_DAY + make_time(h as f64, mi as f64, sec as f64, ms);
    let t = match offset {
        Some(o) => local - (o as f64) * 60_000.0,
        None => local_to_utc(local),
    };
    Some(time_clip(t))
}

fn days_in_month(y: i64, m: i64) -> i64 {
    let next = if m == 12 { NaiveDate::from_ymd_opt(y as i32 + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y as i32, m as u32 + 1, 1) };
    let this = NaiveDate::from_ymd_opt(y as i32, m as u32, 1);
    match (this, next) {
        (Some(a), Some(b)) => (b - a).num_days(),
        _ => 31,
    }
}

// ---------------------------------------------------------------------------
// V8's legacy date parser (src/date/dateparser*.{h,cc}), the parts that
// matter for order dates.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok {
    Num(i64, usize),
    Sym(char),
    Ws,
    Month(i64),
    AmPm(i64),
    Tz(i64), // offset in hours
    Word,
    End,
}

fn tokenize(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            let mut v: i64 = 0;
            while i < chars.len() && chars[i].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add(chars[i] as i64 - '0' as i64);
                i += 1;
            }
            out.push(Tok::Num(v.min(i32::MAX as i64), i - start));
        } else if crate::js::is_ws(c) {
            while i < chars.len() && crate::js::is_ws(chars[i]) {
                i += 1;
            }
            out.push(Tok::Ws);
        } else if c == '(' {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '(' {
                    depth += 1;
                } else if chars[i] == ')' {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
            out.push(keyword(&word));
        } else {
            out.push(Tok::Sym(c));
            i += 1;
        }
    }
    out.push(Tok::End);
    out
}

fn keyword(w: &str) -> Tok {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    if w.len() >= 3 {
        if let Some(i) = MONTHS.iter().position(|m| w.starts_with(m)) {
            return Tok::Month(i as i64 + 1);
        }
    }
    match w {
        "am" => Tok::AmPm(0),
        "pm" => Tok::AmPm(12),
        "ut" | "utc" | "gmt" | "z" => Tok::Tz(0),
        "est" => Tok::Tz(-5),
        "edt" => Tok::Tz(-4),
        "cst" => Tok::Tz(-6),
        "cdt" => Tok::Tz(-5),
        "mst" => Tok::Tz(-7),
        "mdt" => Tok::Tz(-6),
        "pst" => Tok::Tz(-8),
        "pdt" => Tok::Tz(-7),
        _ => Tok::Word,
    }
}

const NONE: i64 = i64::MIN;

fn parse_legacy(s: &str) -> Option<f64> {
    let toks = tokenize(s);
    let mut p = 0;
    let mut day_comp: Vec<i64> = Vec::new();
    let mut named_month = NONE;
    let mut time: Vec<i64> = Vec::new();
    let mut hour_offset = NONE;
    let mut tz_sign = NONE;
    let mut tz_hour = NONE;
    let mut tz_min = NONE;
    let mut has_read_number = false;

    let is_minute = |n: i64| (0..=59).contains(&n);
    let time_expecting = |time: &Vec<i64>, n: i64| {
        (time.len() == 1 && is_minute(n)) || (time.len() == 2 && is_minute(n)) || (time.len() == 3 && (0..=999).contains(&n))
    };
    let peek = |p: usize| toks.get(p).copied().unwrap_or(Tok::End);

    while peek(p) != Tok::End {
        let t = toks[p];
        p += 1;
        match t {
            Tok::Num(n, len) => {
                has_read_number = true;
                if peek(p) == Tok::Sym(':') {
                    p += 1;
                    if peek(p) == Tok::Sym(':') {
                        p += 1;
                        if !time.is_empty() {
                            return None;
                        }
                        time.push(n);
                        time.push(0);
                    } else {
                        if time.len() >= 4 {
                            return None;
                        }
                        time.push(n);
                        if peek(p) == Tok::Sym('.') {
                            p += 1;
                        }
                    }
                } else if peek(p) == Tok::Sym('.') && time_expecting(&time, n) {
                    p += 1;
                    time.push(n);
                    let Tok::Num(msv, mslen) = peek(p) else { return None };
                    p += 1;
                    let ms = if mslen > 3 { msv / 10i64.pow(mslen as u32 - 3) } else { msv * 10i64.pow(3 - mslen as u32) };
                    time.push(ms);
                    while time.len() < 4 {
                        time.push(0);
                    }
                } else if tz_hour != NONE && tz_min == NONE && is_minute(n) {
                    tz_min = n;
                } else if time_expecting(&time, n) {
                    time.push(n);
                    while time.len() < 4 {
                        time.push(0);
                    }
                    match peek(p) {
                        Tok::End | Tok::Ws | Tok::Sym('+') | Tok::Sym('-') => {}
                        Tok::Tz(0) => {}
                        _ => return None,
                    }
                } else {
                    let _ = len;
                    if day_comp.len() >= 3 {
                        return None;
                    }
                    day_comp.push(n);
                    if peek(p) == Tok::Sym('-') {
                        p += 1;
                    }
                }
            }
            Tok::AmPm(v) if !time.is_empty() => hour_offset = v,
            Tok::Month(m) => {
                named_month = m;
                if peek(p) == Tok::Sym('-') {
                    p += 1;
                }
            }
            Tok::Tz(h) if has_read_number => {
                tz_sign = if h < 0 { -1 } else { 1 };
                tz_hour = h.abs();
                tz_min = 0;
            }
            Tok::AmPm(_) | Tok::Tz(_) | Tok::Word => {
                if has_read_number {
                    return None;
                }
                if matches!(peek(p), Tok::Num(..)) {
                    return None;
                }
            }
            Tok::Sym(c) if (c == '+' || c == '-') && ((tz_hour == 0 && tz_min == 0) || !time.is_empty()) => {
                tz_sign = if c == '-' { -1 } else { 1 };
                let (n, length) = match peek(p) {
                    Tok::Num(n, l) => {
                        p += 1;
                        (n, l)
                    }
                    _ => (0, 0),
                };
                has_read_number = true;
                if peek(p) == Tok::Sym(':') {
                    tz_hour = n;
                    tz_min = NONE;
                } else if length == 1 || length == 2 {
                    tz_hour = n;
                    tz_min = 0;
                } else if length == 3 || length == 4 {
                    tz_hour = n / 100;
                    tz_min = n % 100;
                } else {
                    return None;
                }
            }
            Tok::Sym(c) if (c == '+' || c == '-' || c == ')') && has_read_number => return None,
            _ => {}
        }
    }

    // DayComposer::Write
    if day_comp.is_empty() {
        return None;
    }
    while day_comp.len() < 3 {
        day_comp.push(1);
    }
    let is_day = |n: i64| (1..=31).contains(&n);
    let (mut year, month, day);
    if named_month == NONE {
        if !is_day(day_comp[0]) {
            year = day_comp[0];
            month = day_comp[1];
            day = day_comp[2];
        } else {
            month = day_comp[0];
            day = day_comp[1];
            year = day_comp[2];
        }
    } else {
        month = named_month;
        if !is_day(day_comp[0]) {
            year = day_comp[0];
            day = day_comp[1];
        } else {
            day = day_comp[0];
            year = day_comp[1];
        }
    }
    if (0..=49).contains(&year) {
        year += 2000;
    } else if (50..=99).contains(&year) {
        year += 1900;
    }
    if !(1..=12).contains(&month) || !is_day(day) {
        return None;
    }

    // TimeComposer::Write
    while time.len() < 4 {
        time.push(0);
    }
    let (mut hour, minute, second, milli) = (time[0], time[1], time[2], time[3]);
    if hour_offset != NONE {
        if !(0..=12).contains(&hour) {
            return None;
        }
        hour %= 12;
        hour += hour_offset;
    }
    if (!(0..=23).contains(&hour) || !is_minute(minute) || !is_minute(second) || !(0..=999).contains(&milli))
        && (hour != 24 || minute != 0 || second != 0 || milli != 0)
    {
        return None;
    }

    let local = make_day(year as f64, (month - 1) as f64, day as f64) * MS_PER_DAY
        + make_time(hour as f64, minute as f64, second as f64, milli as f64);
    let t = if tz_sign != NONE {
        let h = if tz_hour == NONE { 0 } else { tz_hour };
        let m = if tz_min == NONE { 0 } else { tz_min };
        local - (tz_sign * (h * 60 + m)) as f64 * 60_000.0
    } else {
        local_to_utc(local)
    };
    time_clip(t)
}

/// `new Date(s).getTime()`, None for an Invalid Date.
pub fn parse(s: &str) -> Option<f64> {
    if let Some(r) = parse_iso(s) {
        return r;
    }
    parse_legacy(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_dates() {
        let t = parse("August 1, 2026").unwrap();
        let p = local_parts(t);
        assert_eq!((p.year, p.month0, p.day), (2026, 7, 1));
        let t = parse("Ordered on August 1, 2026").unwrap();
        assert_eq!(local_parts(t).month0, 7);
        let t = parse("1 August 2026").unwrap();
        assert_eq!(local_parts(t).day, 1);
        assert!(parse("Unknown").is_none());
        assert!(parse("").is_none());
        assert_eq!(parse("2026-08-01"), Some(1_785_542_400_000.0));
        assert_eq!(parse("2026-08-01T00:00:00Z"), Some(1_785_542_400_000.0));
        assert!(parse("2026-13-01").is_none());
        let t = parse("January 15, 2020").unwrap();
        assert_eq!(local_parts(t).year, 2020);
    }
}

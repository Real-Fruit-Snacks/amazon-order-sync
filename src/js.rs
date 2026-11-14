//! The JavaScript behaviour the notes depend on, so the Rust server writes the
//! same bytes the Node server did: String() and Number() coercion, trim, the
//! number-to-text rules, toFixed, Math.round, parseFloat/parseInt, UTF-16
//! lengths and slices, object key order, and JS regular expressions.

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Whitespace and trim (String.prototype.trim: WhiteSpace + LineTerminator)
// ---------------------------------------------------------------------------

pub fn is_ws(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_ws)
}

// ---------------------------------------------------------------------------
// UTF-16 lengths and slices (JS string indices count UTF-16 code units)
// ---------------------------------------------------------------------------

pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` in UTF-16 units. A surrogate pair cut in half becomes
/// U+FFFD, which is what Node writes for the lone surrogate JS would keep.
pub fn utf16_prefix(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.len_utf16();
        if used + w > n {
            if used < n {
                out.push('\u{FFFD}');
            }
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

// ---------------------------------------------------------------------------
// Number -> string (Number::toString, radix 10)
// ---------------------------------------------------------------------------

pub fn num_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let neg = x < 0.0;
    let a = x.abs();
    // Rust's {:e} is the shortest round-trip representation: d.ddddde±x.
    let sci = format!("{:e}", a);
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let k = digits.len() as i32;
    let n = exp + 1; // position of the decimal point
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let e = n - 1;
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// A JSON number the way JSON.stringify would write it: integers without a
/// fraction, NaN and the infinities as null.
pub fn jnum(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x.fract() == 0.0 && x.abs() < 9.0e18 {
        return Value::from(x as i64);
    }
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// String() and Number() coercion of JSON values. `None` is `undefined`.
// ---------------------------------------------------------------------------

pub fn to_str(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => num_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|e| match e {
                Value::Null => String::new(),
                other => to_str(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `String(v ?? '')`
pub fn to_str_or_empty(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        other => to_str(other),
    }
}

pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        Some(Value::Array(_)) => string_to_number(&to_str(v)),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// The length of the StrDecimalLiteral prefix of `s` (no leading whitespace),
/// Infinity included. 0 when there is none.
fn decimal_literal_prefix(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return i + 8;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        let frac_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let frac = j - frac_start;
        if digits > 0 || frac > 0 {
            i = j;
            digits += frac;
        }
    }
    if digits == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

fn parse_decimal(lit: &str) -> f64 {
    let (neg, body) = match lit.as_bytes().first() {
        Some(b'-') => (true, &lit[1..]),
        Some(b'+') => (false, &lit[1..]),
        _ => (false, lit),
    };
    let v = if body == "Infinity" {
        f64::INFINITY
    } else {
        let fixed = if body.starts_with('.') { format!("0{}", body) } else { body.to_string() };
        let fixed = if fixed.ends_with('.') { format!("{}0", fixed) } else { fixed };
        fixed.parse::<f64>().unwrap_or(f64::NAN)
    };
    if neg {
        -v
    } else {
        v
    }
}

pub fn string_to_number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.as_bytes();
    if lower.len() > 2 && lower[0] == b'0' {
        let radix = match lower[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let digits = &t[2..];
            if digits.chars().all(|c| c.is_digit(radix)) {
                let mut v = 0f64;
                for c in digits.chars() {
                    v = v * radix as f64 + c.to_digit(radix).unwrap() as f64;
                }
                return v;
            }
            return f64::NAN;
        }
    }
    let n = decimal_literal_prefix(t);
    if n != t.len() {
        return f64::NAN;
    }
    parse_decimal(t)
}

pub fn parse_float(s: &str) -> f64 {
    let t = trim_start(s);
    let n = decimal_literal_prefix(t);
    if n == 0 {
        return f64::NAN;
    }
    parse_decimal(&t[..n])
}

/// `Number.parseInt(String(v), 10)`
pub fn parse_int10(s: &str) -> f64 {
    let t = trim_start(s);
    let b = t.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == start {
        return f64::NAN;
    }
    let v: f64 = t[start..i].parse().unwrap_or(f64::NAN);
    if neg {
        -v
    } else {
        v
    }
}

pub fn is_integer(x: f64) -> bool {
    x.is_finite() && x.trunc() == x
}

/// `Number.isInteger(v)` on a JSON value: only real numbers qualify.
pub fn json_is_int(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => {
            let f = n.as_f64()?;
            if is_integer(f) {
                Some(f as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn json_num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

pub fn math_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// Number.prototype.toFixed: the exact decimal value of the double, ties
/// rounded to the larger magnitude.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() {
        return num_to_string(x);
    }
    if x.abs() >= 1e21 {
        return num_to_string(x);
    }
    let neg = x < 0.0;
    // Every double has a finite decimal expansion of at most 1074 places.
    let exact = format!("{:.1100}", x.abs());
    let (int_part, frac_part) = exact.split_once('.').unwrap();
    let mut digits_vec: Vec<u8> = int_part.bytes().collect();
    let keep = &frac_part[..digits];
    digits_vec.extend(keep.bytes());
    let rest = &frac_part[digits..];
    let round_up = rest.as_bytes()[0] >= b'5';
    if round_up {
        let mut i = digits_vec.len();
        loop {
            if i == 0 {
                digits_vec.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits_vec[i] == b'9' {
                digits_vec[i] = b'0';
            } else {
                digits_vec[i] += 1;
                break;
            }
        }
    }
    let s = String::from_utf8(digits_vec).unwrap();
    let split = s.len() - digits;
    let mut out = String::new();
    let int_str = &s[..split];
    let is_zero = s.bytes().all(|b| b == b'0');
    if neg && !is_zero {
        out.push('-');
    }
    out.push_str(int_str);
    if digits > 0 {
        out.push('.');
        out.push_str(&s[split..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Object key order. JS objects list array-index keys first (ascending), then
// the rest in insertion order. normalize() applies that, and turns integral
// floats into integers so they print the way JSON.stringify prints them.
// ---------------------------------------------------------------------------

fn array_index(k: &str) -> Option<u64> {
    if k == "0" {
        return Some(0);
    }
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 10 || b[0] == b'0' || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v: u64 = k.parse().ok()?;
    if v <= 4_294_967_294 {
        Some(v)
    } else {
        None
    }
}

pub fn order_object(m: &mut Map<String, Value>) {
    if !m.keys().any(|k| array_index(k).is_some()) {
        return;
    }
    let old = std::mem::take(m);
    let mut idx: Vec<(u64, String, Value)> = Vec::new();
    let mut rest: Vec<(String, Value)> = Vec::new();
    for (k, v) in old {
        match array_index(&k) {
            Some(i) => idx.push((i, k, v)),
            None => rest.push((k, v)),
        }
    }
    idx.sort_by_key(|e| e.0);
    for (_, k, v) in idx {
        m.insert(k, v);
    }
    for (k, v) in rest {
        m.insert(k, v);
    }
}

pub fn normalize(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if !n.is_i64() && !n.is_u64() {
                    *v = jnum(f);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        Value::Object(m) => {
            order_object(m);
            m.values_mut().for_each(normalize);
        }
        _ => {}
    }
}

/// JSON.parse: the parsed value with JS key order and number shapes.
pub fn json_parse(s: &str) -> Option<Value> {
    let mut v: Value = serde_json::from_str(s).ok()?;
    normalize(&mut v);
    Some(v)
}

/// JSON.stringify(v)
pub fn stringify(v: &Value) -> String {
    let mut v = v.clone();
    normalize(&mut v);
    serde_json::to_string(&v).unwrap_or_else(|_| "null".into())
}

/// JSON.stringify(v, null, 2)
pub fn stringify_pretty(v: &Value) -> String {
    let mut v = v.clone();
    normalize(&mut v);
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "null".into())
}

// ---------------------------------------------------------------------------
// Regular expressions with JS semantics, translated for fancy-regex: `.`
// stops at every JS line terminator, \s/\d/\w are the JS classes, and ^/$
// follow the m flag the way JS does (no flag: whole input only).
// ---------------------------------------------------------------------------

const WS_CLASS: &str = "\\t\\n\\x0B\\x0C\\r \\x{A0}\\x{1680}\\x{2000}-\\x{200A}\\x{2028}\\x{2029}\\x{202F}\\x{205F}\\x{3000}\\x{FEFF}";
const LT_CLASS: &str = "\\n\\r\\x{2028}\\x{2029}";

pub fn translate(src: &str, multiline: bool) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_class = false;
    let mut class_buf = String::new();
    while i < chars.len() {
        let c = chars[i];
        if in_class {
            if c == '\\' && i + 1 < chars.len() {
                let n = chars[i + 1];
                match n {
                    's' => class_buf.push_str(WS_CLASS),
                    'S' => class_buf.push_str("\u{0}NOTWS\u{0}"),
                    'd' => class_buf.push_str("0-9"),
                    'w' => class_buf.push_str("A-Za-z0-9_"),
                    '/' => class_buf.push('/'),
                    _ => {
                        class_buf.push('\\');
                        class_buf.push(n);
                    }
                }
                i += 2;
                continue;
            }
            if c == ']' {
                in_class = false;
                if class_buf.contains("\u{0}NOTWS\u{0}") {
                    // [\s\S] (and friends): any character at all.
                    out.push_str("(?s:.)");
                } else {
                    out.push('[');
                    out.push_str(&class_buf);
                    out.push(']');
                }
                class_buf.clear();
                i += 1;
                continue;
            }
            if c == '[' {
                class_buf.push_str("\\[");
            } else {
                class_buf.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '\\' if i + 1 < chars.len() => {
                let n = chars[i + 1];
                match n {
                    's' => out.push_str(&format!("[{}]", WS_CLASS)),
                    'S' => out.push_str(&format!("[^{}]", WS_CLASS)),
                    'd' => out.push_str("[0-9]"),
                    'D' => out.push_str("[^0-9]"),
                    'w' => out.push_str("[A-Za-z0-9_]"),
                    'W' => out.push_str("[^A-Za-z0-9_]"),
                    '/' => out.push('/'),
                    '-' => out.push_str("\\-"),
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
                i += 2;
            }
            '[' => {
                in_class = true;
                if i + 1 < chars.len() && chars[i + 1] == '^' {
                    class_buf.push('^');
                    i += 1;
                }
                i += 1;
            }
            '.' => {
                out.push_str(&format!("[^{}]", LT_CLASS));
                i += 1;
            }
            '^' => {
                if multiline {
                    out.push_str(&format!("(?:\\A|(?<=[{}]))", LT_CLASS));
                } else {
                    out.push_str("\\A");
                }
                i += 1;
            }
            '$' => {
                if multiline {
                    out.push_str(&format!("(?=[{}]|\\z)", LT_CLASS));
                } else {
                    out.push_str("\\z");
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

pub type Re = fancy_regex::Regex;

fn cache() -> &'static Mutex<HashMap<(String, String), &'static Re>> {
    static C: OnceLock<Mutex<HashMap<(String, String), &'static Re>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compile (once) a JS regex source with JS flags (`m`, `i`; `g` is the
/// caller's business).
pub fn re(src: &str, flags: &str) -> &'static Re {
    let key = (src.to_string(), flags.to_string());
    let mut c = cache().lock().unwrap();
    if let Some(r) = c.get(&key) {
        return r;
    }
    let mut pat = String::new();
    if flags.contains('i') {
        pat.push_str("(?i)");
    }
    pat.push_str(&translate(src, flags.contains('m')));
    let compiled = fancy_regex::RegexBuilder::new(&pat)
        .backtrack_limit(10_000_000)
        .build()
        .unwrap_or_else(|e| panic!("bad regex {src:?} -> {pat:?}: {e}"));
    let leaked: &'static Re = Box::leak(Box::new(compiled));
    c.insert(key, leaked);
    leaked
}

pub struct Match {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
}

impl Match {
    pub fn group<'a>(&self, s: &'a str, i: usize) -> Option<&'a str> {
        self.groups.get(i).copied().flatten().map(|(a, b)| &s[a..b])
    }
    pub fn text<'a>(&self, s: &'a str) -> &'a str {
        &s[self.start..self.end]
    }
}

pub fn find_at(r: &Re, s: &str, from: usize) -> Option<Match> {
    let caps = r.captures_from_pos(s, from).ok().flatten()?;
    let whole = caps.get(0)?;
    let groups = (0..caps.len()).map(|i| caps.get(i).map(|m| (m.start(), m.end()))).collect();
    Some(Match { start: whole.start(), end: whole.end(), groups })
}

pub fn find(r: &Re, s: &str) -> Option<Match> {
    find_at(r, s, 0)
}

pub fn test(r: &Re, s: &str) -> bool {
    r.is_match(s).unwrap_or(false)
}

/// All non-overlapping matches (a /g scan). Empty matches advance by one char.
pub fn find_all(r: &Re, s: &str) -> Vec<Match> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= s.len() {
        let Some(m) = find_at(r, s, pos) else { break };
        let next = if m.end == m.start { s[m.end..].chars().next().map(|c| m.end + c.len_utf8()).unwrap_or(s.len() + 1) } else { m.end };
        out.push(m);
        pos = next;
    }
    out
}

/// `s.replace(re, f)` (first match) or with /g every match.
pub fn replace_with(r: &Re, s: &str, global: bool, mut f: impl FnMut(&Match, &str) -> String) -> String {
    let ms = if global { find_all(r, s) } else { find(r, s).into_iter().collect() };
    let mut out = String::new();
    let mut last = 0;
    for m in &ms {
        out.push_str(&s[last..m.start]);
        out.push_str(&f(m, s));
        last = m.end;
    }
    out.push_str(&s[last..]);
    out
}

pub fn replace_str(r: &Re, s: &str, global: bool, with: &str) -> String {
    replace_with(r, s, global, |_, _| with.to_string())
}

/// `s.split(re)`; capturing groups land in the output like JS.
pub fn split(r: &Re, s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in find_all(r, s) {
        if m.end == m.start && (m.start == 0 || m.start == s.len()) {
            continue;
        }
        out.push(s[last..m.start].to_string());
        for (a, b) in m.groups.iter().skip(1).flatten() {
            out.push(s[*a..*b].to_string());
        }
        last = m.end;
    }
    out.push(s[last..].to_string());
    out
}

/// String.prototype.lastIndexOf(needle, from) on byte offsets.
pub fn last_index_of(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let limit = (from + needle.len()).min(hay.len());
    let mut lim = limit;
    while !hay.is_char_boundary(lim) {
        lim -= 1;
    }
    hay[..lim].rfind(needle)
}

pub fn index_of(hay: &str, needle: &str, from: usize) -> Option<usize> {
    if from > hay.len() {
        return if needle.is_empty() { Some(hay.len()) } else { None };
    }
    let mut f = from;
    while !hay.is_char_boundary(f) {
        f += 1;
    }
    hay[f..].find(needle).map(|i| i + f)
}

// ---------------------------------------------------------------------------
// URI component coding
// ---------------------------------------------------------------------------

pub fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let h = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let v = u8::from_str_radix(h, 16).ok()?;
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn encode_uri_component(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_js() {
        assert_eq!(num_to_string(12.5), "12.5");
        assert_eq!(num_to_string(1234.0), "1234");
        assert_eq!(num_to_string(0.000001), "0.000001");
        assert_eq!(num_to_string(1e-7), "1e-7");
        assert_eq!(num_to_string(1e21), "1e+21");
        assert_eq!(num_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(num_to_string(-0.5), "-0.5");
    }

    #[test]
    fn to_fixed_matches_js() {
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(25.0, 2), "25.00");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(99.999, 2), "100.00");
        assert_eq!(to_fixed(-0.001, 2), "0.00");
    }

    #[test]
    fn coercion() {
        assert_eq!(string_to_number(" 12 "), 12.0);
        assert_eq!(string_to_number(""), 0.0);
        assert!(string_to_number("12abc").is_nan());
        assert_eq!(string_to_number("0x1A"), 26.0);
        assert_eq!(string_to_number("1e3"), 1000.0);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("5."), 5.0);
        assert_eq!(parse_float("12.99.3"), 12.99);
        assert!(parse_float("abc").is_nan());
        assert_eq!(parse_int10("  3 items"), 3.0);
        assert!(parse_int10("undefined").is_nan());
        assert_eq!(math_round(2.5), 3.0);
        assert_eq!(math_round(-2.5), -2.0);
        assert_eq!(math_round(0.49999999999999994), 0.0);
    }

    #[test]
    fn regex_translation() {
        let r = re(r"^status:\s*(.*?)\s*$", "m");
        let s = "a: 1\r\nstatus: Delivered\r\nb: 2";
        let m = find(r, s).unwrap();
        assert_eq!(m.group(s, 1), Some("Delivered"));
        let r = re(r"---[\s\S]*?---", "");
        assert!(test(r, "---\nx\n---"));
        let r = re(r"\*\*Rating:\*\*\s*([1-5])(?!\d)", "");
        let s = "**Rating:** 45\n**Rating:** 4";
        assert_eq!(find(r, s).unwrap().group(s, 1), Some("4"));
        assert_eq!(find(r, s).unwrap().start, 15);
    }

    #[test]
    fn split_keeps_captures() {
        let r = re(r"(\r?\n---\r?\n)", "");
        let parts = split(r, "a\n---\nb\r\n---\r\nc");
        assert_eq!(parts, vec!["a", "\n---\n", "b", "\r\n---\r\n", "c"]);
    }

    #[test]
    fn key_order() {
        let v = json_parse(r#"{"b":1,"2":2,"a":3,"1":4,"0123":5}"#).unwrap();
        assert_eq!(stringify(&v), r#"{"1":4,"2":2,"b":1,"a":3,"0123":5}"#);
        let v = json_parse("{\"x\":1.0,\"y\":2.5}").unwrap();
        assert_eq!(stringify(&v), r#"{"x":1,"y":2.5}"#);
    }
}

//! The order note format: building a note from a sync payload, reading one
//! back, and the in-place edits (review block, checkbox, frontmatter arrays).
//! Pure functions; the store does the file work.

use crate::js::{self, find, re, to_str, to_str_or_empty, Match};
use serde_json::{json, Map, Value};

pub const REVIEW_HEADER: &str = "#### 📝 My Review";
pub const REVIEW_LINK: &str = "[✍️ Leave a Review on Amazon]";
pub const PLACEHOLDER: &str = "> *Write your review for this item below...*\n**Rating:** \n**Review:** ";

/// Property access on a JSON value: anything but an object yields undefined.
pub fn prop<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|m| m.get(key))
}

// ---------------------------------------------------------------------------
// JS replacement patterns ($$, $&, $`, $', $n) for string replacements
// ---------------------------------------------------------------------------

pub fn expand_replacement(repl: &str, m: &Match, s: &str) -> String {
    let b: Vec<char> = repl.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '$' && i + 1 < b.len() {
            let n = b[i + 1];
            match n {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    out.push_str(m.text(s));
                    i += 2;
                    continue;
                }
                '`' => {
                    out.push_str(&s[..m.start]);
                    i += 2;
                    continue;
                }
                '\'' => {
                    out.push_str(&s[m.end..]);
                    i += 2;
                    continue;
                }
                d if d.is_ascii_digit() => {
                    let groups = m.groups.len() - 1;
                    // Two-digit group first when it exists.
                    if i + 2 < b.len() && b[i + 2].is_ascii_digit() {
                        let nn = (d as usize - '0' as usize) * 10 + (b[i + 2] as usize - '0' as usize);
                        if nn >= 1 && nn <= groups {
                            out.push_str(m.group(s, nn).unwrap_or(""));
                            i += 3;
                            continue;
                        }
                    }
                    let n1 = d as usize - '0' as usize;
                    if n1 >= 1 && n1 <= groups {
                        out.push_str(m.group(s, n1).unwrap_or(""));
                        i += 2;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `s.replace(regex, string)` for the first match, with JS `$` patterns.
pub fn replace_first(src: &str, flags: &str, s: &str, repl: &str) -> String {
    let r = re(src, flags);
    match find(r, s) {
        Some(m) => {
            let mut out = s[..m.start].to_string();
            out.push_str(&expand_replacement(repl, &m, s));
            out.push_str(&s[m.end..]);
            out
        }
        None => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Frontmatter arrays
// ---------------------------------------------------------------------------

/// Split the inside of an inline `[ ... ]` array on commas outside quotes.
fn split_inline_array(inner: &str) -> Vec<String> {
    let chars: Vec<char> = inner.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote = '\0';
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if in_quotes {
            if ch == '\\' && i + 1 < chars.len() {
                current.push(ch);
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == quote {
                in_quotes = false;
            }
            current.push(ch);
        } else if ch == '"' || ch == '\'' {
            in_quotes = true;
            quote = ch;
            current.push(ch);
        } else if ch == ',' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
        i += 1;
    }
    parts.push(current);
    parts
}

/// Quote-aware: finds `field: [` and the matching `]` outside double quotes.
fn extract_inline_array_inner<'a>(content: &'a str, field: &str) -> Option<&'a str> {
    let head = find(re(&format!(r"{}:\s*\[", field), ""), content)?;
    let start = head.end;
    let b = content.as_bytes();
    let mut in_quotes = false;
    let mut i = start;
    while i < b.len() {
        let ch = b[i];
        if in_quotes {
            if ch == b'\\' && i + 1 < b.len() {
                // Skip the escaped character (a whole UTF-8 sequence).
                i += 1;
                while i + 1 < b.len() && !content.is_char_boundary(i + 1) {
                    i += 1;
                }
                i += 1;
                continue;
            }
            if ch == b'"' {
                in_quotes = false;
            }
        } else if ch == b'"' {
            in_quotes = true;
        } else if ch == b']' {
            return Some(&content[start..i]);
        }
        i += 1;
    }
    None
}

fn unescape_inline_value(s: &str) -> String {
    let once = js::replace_with(re(r"\\(.)", "g"), s, true, |m, s| m.group(s, 1).unwrap_or("").to_string());
    let unq = js::replace_str(re(r#"['"]"#, "g"), &once, true, "");
    js::trim(&unq).to_string()
}

/// Filtering parser: drops empty entries. Only for existence checks and the
/// `asin` array (which never has empty entries).
pub fn parse_array_field(content: &str, field: &str) -> Vec<String> {
    if let Some(inner) = extract_inline_array_inner(content, field) {
        return split_inline_array(inner).iter().map(|p| unescape_inline_value(p)).filter(|v| !v.is_empty()).collect();
    }
    let ml = re(&format!(r"^{}:\s*\r?\n((?:\s+-\s+.*\r?\n?)*)", field), "m");
    if let Some(m) = find(ml, content) {
        let lines_str = m.group(content, 1).unwrap_or("");
        let mut result = Vec::new();
        for line in js::split(re(r"\r?\n", ""), lines_str) {
            if js::trim(&line).starts_with("---") {
                continue;
            }
            if let Some(im) = find(re(r"^\s+-\s+(.*)$", ""), &line) {
                let raw = im.group(&line, 1).unwrap_or("");
                let val = js::replace_str(re(r#"['"]"#, "g"), raw, true, "");
                let val = js::trim(&val).to_string();
                if !val.is_empty() {
                    result.push(val);
                }
            }
        }
        return result;
    }
    let single = re(&format!(r#"^{}:\s*['"]?(.*?)['"]?\s*\r?$"#, field), "m");
    if let Some(m) = find(single, content) {
        let v = m.group(content, 1).unwrap_or("");
        if !v.starts_with('[') {
            return vec![js::trim(v).to_string()];
        }
    }
    Vec::new()
}

/// Alignment-preserving parser for fields that hold "" entries.
pub fn parse_aligned_array(content: &str, field: &str) -> Vec<String> {
    let Some(inner) = extract_inline_array_inner(content, field) else { return Vec::new() };
    if js::trim(inner).is_empty() {
        return Vec::new();
    }
    split_inline_array(inner).iter().map(|p| unescape_inline_value(p)).collect()
}

/// One double-quoted YAML value: CR/LF become a space, `\` and `"` escaped.
pub fn yaml_escape(s: &str) -> String {
    let flat = js::replace_str(re(r"[\r\n]+", "g"), s, true, " ");
    format!("\"{}\"", flat.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn yaml_escape_v(v: Option<&Value>) -> String {
    yaml_escape(&to_str_or_empty(v))
}

// ---------------------------------------------------------------------------
// Ids and small helpers
// ---------------------------------------------------------------------------

pub fn sanitize_id_str(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('_');
            }
        }
    }
    out
}

pub fn sanitize_id(v: Option<&Value>) -> String {
    sanitize_id_str(&to_str_or_empty(v))
}

pub fn is_safe_id(s: &str) -> bool {
    !s.is_empty() && !s.contains("..")
}

fn first_unit(c: char) -> u32 {
    let mut buf = [0u16; 2];
    c.encode_utf16(&mut buf)[0] as u32
}

pub fn default_rating_for_asin(asin: &str) -> i64 {
    let sum: u64 = asin.chars().map(|c| first_unit(c) as u64).sum();
    if sum % 10 < 4 {
        4
    } else {
        5
    }
}

pub const REVIEW_SHAPES: [&str; 6] = [
    "Open with the single most useful concrete detail (a measurement, a texture, a behaviour you noticed), then widen out. Put the drawback in the middle. No closing recommendation; end on a specific observation.",
    "Open with the drawback, stated plainly in one sentence, then explain why the product still earns its rating. End with who it suits.",
    "Open with a specific moment of using it (where, when, what for). Keep paragraphs short. Mention the drawback in passing inside a sentence about something else. End without summarising.",
    "Compare it to what you used before or expected (without naming other brands or products). Lead with the difference that mattered most. Drawback near the end, one sentence, no softening.",
    "Short and direct: four to seven sentences total, no filler, each sentence carrying a fact. Title is a blunt verdict with one detail. Drawback is one clause.",
    "Structure around who it is for and who should skip it. Make the first sentence a scenario. Weave two concrete specs into the middle. Finish on the \"skip it if\" note rather than praise.",
];

pub fn seed_from_string(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for c in s.chars() {
        h ^= first_unit(c);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

pub fn shape_for_seed(seed: u32) -> &'static str {
    REVIEW_SHAPES[(seed as usize) % REVIEW_SHAPES.len()]
}

pub fn pick_rotating<T: Clone>(pool: &[T], count: usize, seed: u32) -> Vec<T> {
    if pool.is_empty() {
        return Vec::new();
    }
    let n = count.min(pool.len());
    let start = (seed as usize) % pool.len();
    (0..n).map(|i| pool[(start + i) % pool.len()].clone()).collect()
}

/// Loose price parser; None when unparseable (unknown, not $0.00).
pub fn parse_price_number(v: Option<&Value>) -> Option<f64> {
    let Some(Value::String(s)) = v else { return None };
    parse_price_str(s)
}

pub fn parse_price_str(s: &str) -> Option<f64> {
    if js::trim(s).is_empty() {
        return None;
    }
    let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    if cleaned.is_empty() || !cleaned.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let last_dot = cleaned.rfind('.');
    let last_comma = cleaned.rfind(',');
    let normalized = match (last_dot, last_comma) {
        (Some(d), Some(c)) => {
            if d > c {
                cleaned.replace(',', "")
            } else {
                cleaned.replace('.', "").replacen(',', ".", 1)
            }
        }
        (None, Some(c)) => {
            let trailing = cleaned.len() - c - 1;
            if (1..=2).contains(&trailing) && cleaned.find(',') == Some(c) {
                cleaned.replacen(',', ".", 1)
            } else {
                cleaned.replace(',', "")
            }
        }
        _ => cleaned,
    };
    let n = js::parse_float(&normalized);
    if n.is_nan() {
        None
    } else {
        Some(n)
    }
}

/// Non-negative integer or None (helpful_votes): Number() coercion first.
pub fn non_negative_int(v: Option<&Value>) -> Option<f64> {
    let n = js::to_number(v);
    if js::is_integer(n) && n >= 0.0 {
        Some(n + 0.0)
    } else {
        None
    }
}

pub fn non_negative_int_str(s: Option<&str>) -> Option<f64> {
    match s {
        None => None,
        Some(s) => {
            let n = js::string_to_number(s);
            if js::is_integer(n) && n >= 0.0 {
                Some(n + 0.0)
            } else {
                None
            }
        }
    }
}

pub fn context_asin_ok(s: &str) -> bool {
    (6..=12).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The assets/ path a cached image would get for this URL + ASIN.
pub fn image_path_from_url(url: Option<&Value>, asin: Option<&Value>) -> Option<Value> {
    let truthy = match url {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        _ => true,
    };
    if !truthy || !to_str(url).starts_with("http") {
        return url.cloned();
    }
    let u = to_str(url);
    let ext =
        find(re(r"\.([a-zA-Z0-9]+)(?:[?#]|$)", ""), &u).map(|m| m.group(&u, 1).unwrap_or("").to_string()).unwrap_or_else(|| "jpg".into());
    Some(Value::String(format!("assets/{}.{}", sanitize_id(asin), ext)))
}

pub fn safe_review_text(s: &str) -> String {
    js::replace_str(re(r"^[ \t]*-{3,}[ \t]*$", "gm"), s, true, "- - -")
}

pub fn without_placeholder(block: &str) -> String {
    let r = re(r"^> \*Write your review for this item below\.\.\.\*[ \t]*\r?\n?", "m");
    let out = js::replace_str(r, block, false, "");
    js::trim(&out).to_string()
}

pub fn has_review_content(block: &str) -> bool {
    js::test(re(r"\*\*(Title|Review):\*\*[ \t]*\S", ""), block)
}

// ---------------------------------------------------------------------------
// Canned drafts
// ---------------------------------------------------------------------------

pub const FALLBACK_REVIEWS: [(&str, &str); 5] = [
    ("Did not work out for me", "I wanted to like this, but it fell short in the ways that matter most for how I use it. The core issues outweighed anything positive, so I would not buy it again."),
    ("Fell short of expectations", "It technically works, but the execution has enough rough edges that I kept noticing them in daily use. I would look at alternatives before buying this one again."),
    ("Decent, with real caveats", "Mixed feelings on this one. Some parts work well and others feel average, so it is fine for what it is but not a standout. Worth it only if the tradeoffs fit your needs."),
    ("Good overall, one small gripe", "Happy with it overall: it handles the main job well and feels well made. There is one small thing I would tweak, but it does not change my recommendation."),
    ("Very happy with this one", "Really happy with this purchase. It does exactly what I needed, the quality feels right for daily use, and I have no real complaints so far."),
];

pub fn normalize_rating(r: f64) -> i64 {
    if js::is_integer(r) && (1.0..=5.0).contains(&r) {
        r as i64
    } else {
        5
    }
}

pub fn fallback_review(rating: f64) -> (String, String) {
    let (t, x) = FALLBACK_REVIEWS[(normalize_rating(rating) - 1) as usize];
    (t.to_string(), x.to_string())
}

pub fn is_template_draft(block: &str) -> bool {
    let Some(t) = find(re(r"\*\*Title:\*\*\s*(.*)", ""), block) else { return false };
    let Some(x) = find(re(r"\*\*Review:\*\*\s*([\s\S]*)$", ""), block) else { return false };
    let title = js::trim(t.group(block, 1).unwrap_or(""));
    let body = js::trim(x.group(block, 1).unwrap_or(""));
    FALLBACK_REVIEWS.iter().any(|(ft, fx)| *ft == title && *fx == body)
}

// ---------------------------------------------------------------------------
// Building a note
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct ItemExtra {
    /// None = undefined (derive from the URL); Some(Null) behaves the same.
    pub image: Option<Value>,
    pub review: Option<String>,
    pub posted: bool,
    pub rating: Option<Value>,
    pub helpful_votes: Option<f64>,
    pub tone: String,
}

fn nullish(v: &Option<Value>) -> bool {
    matches!(v, None | Some(Value::Null))
}

pub fn build_note(payload: &Value, extras: &[ItemExtra]) -> String {
    let empty = Vec::new();
    let items = match prop(payload, "items") {
        Some(Value::Array(a)) => a,
        _ => &empty,
    };
    let mut brands = Vec::new();
    let mut prices = Vec::new();
    let mut asins = Vec::new();
    let mut urls = Vec::new();
    let mut revieweds = Vec::new();
    let mut ratings = Vec::new();
    let mut votes_out = Vec::new();
    let mut any_votes = false;
    let mut tones = Vec::new();
    let mut any_tone = false;
    let mut items_content = String::new();
    let default_extra = ItemExtra::default();
    for (i, item) in items.iter().enumerate() {
        let extra = extras.get(i).unwrap_or(&default_extra);
        brands.push(yaml_escape_v(prop(item, "brand")));
        prices.push(yaml_escape_v(prop(item, "current_price")));
        asins.push(yaml_escape_v(prop(item, "asin")));
        urls.push(yaml_escape_v(prop(item, "link")));
        let posted = extra.posted;
        revieweds.push(if posted { "true" } else { "false" });
        let rating = if !nullish(&extra.rating) { extra.rating.clone() } else { prop(item, "review_rating").cloned() };
        ratings.push(yaml_escape_v(rating.as_ref()));
        let votes = extra.helpful_votes.or_else(|| non_negative_int(prop(item, "helpful_votes")));
        if votes.is_some() {
            any_votes = true;
        }
        votes_out.push(js::num_to_string(votes.unwrap_or(0.0)));
        if !extra.tone.is_empty() {
            any_tone = true;
        }
        tones.push(yaml_escape(&extra.tone));
        let image = if !nullish(&extra.image) { extra.image.clone() } else { image_path_from_url(prop(item, "image"), prop(item, "asin")) };
        let review = extra.review.clone().unwrap_or_else(|| PLACEHOLDER.to_string());
        items_content.push_str(&format!(
            "\n### {}\n\n![image]({})\n\n#### Features\n{}\n\n{}\n{}(https://www.amazon.com/review/create-review?asin={})\n\n{}\n- [{}] Posted to Amazon?\n\n---\n",
            to_str(prop(item, "name")),
            to_str(image.as_ref()),
            to_str(prop(item, "features")),
            REVIEW_HEADER,
            REVIEW_LINK,
            to_str(prop(item, "asin")),
            review,
            if posted { "x" } else { " " },
        ));
    }
    let votes_line = if any_votes { format!("helpful_votes: [{}]\n", votes_out.join(", ")) } else { String::new() };
    let tone_line = if any_tone { format!("tone: [{}]\n", tones.join(", ")) } else { String::new() };
    let backfill_line = if prop(payload, "backfill") == Some(&Value::Bool(true)) { "backfill: true\n" } else { "" };
    format!(
        "---\ntype: order\norder_id: {}\ndate: {}\ntotal: {}\nstatus: {}\nship_to: {}\n{}brand: [{}]\nprice: [{}]\nasin: [{}]\nurl: [{}]\nreviewed: [{}]\nrating: [{}]\n{}{}---\n# Order {}\n\n**Order Date:** {}\n**Total:** {}\n**Status:** {}\n**Ship To:** {}\n\n---\n\n## 📦 Items\n{}\n",
        yaml_escape_v(prop(payload, "order_id")),
        yaml_escape_v(prop(payload, "date")),
        yaml_escape_v(prop(payload, "total")),
        yaml_escape_v(prop(payload, "status")),
        yaml_escape_v(prop(payload, "ship_to")),
        backfill_line,
        brands.join(", "),
        prices.join(", "),
        asins.join(", "),
        urls.join(", "),
        revieweds.join(", "),
        ratings.join(", "),
        votes_line,
        tone_line,
        to_str(prop(payload, "order_id")),
        to_str(prop(payload, "date")),
        to_str(prop(payload, "total")),
        to_str(prop(payload, "status")),
        to_str(prop(payload, "ship_to")),
        items_content,
    )
}

// ---------------------------------------------------------------------------
// Reading a note
// ---------------------------------------------------------------------------

pub fn scalar_field(fm: &str, name: &str) -> String {
    let q = re(&format!(r#"^{}:\s*"((?:\\.|[^"\\])*)"\s*$"#, name), "m");
    if let Some(m) = find(q, fm) {
        let inner = m.group(fm, 1).unwrap_or("");
        return js::replace_with(re(r#"\\(["'\\])"#, "g"), inner, true, |mm, s| mm.group(s, 1).unwrap_or("").to_string());
    }
    let p = re(&format!(r"^{}:\s*(.*?)\s*$", name), "m");
    match find(p, fm) {
        Some(m) => m.group(fm, 1).unwrap_or("").to_string(),
        None => String::new(),
    }
}

pub fn frontmatter(content: &str) -> Option<String> {
    let m = find(re(r"^---\r?\n([\s\S]*?)\r?\n---", ""), content)?;
    Some(m.group(content, 1).unwrap_or("").to_string())
}

pub fn parse_order_date(s: &str) -> Option<f64> {
    crate::jsdate::parse(&s.replacen("Ordered on ", "", 1))
}

const POSTED_RE: &str = r"- \[\s*[xX]\s*\]\s*Posted to Amazon\?";

pub fn is_posted_block(block: &str) -> bool {
    js::test(re(POSTED_RE, ""), block)
}

pub struct ParsedNote {
    pub order: Value,
    pub sort_key: f64,
}

/// One note -> the /api/orders order object (no absolute paths).
pub fn parse_note(content: &str, rel_path: &str, modified: f64, image_for: &dyn Fn(&str) -> Option<String>) -> Option<ParsedNote> {
    let fm = frontmatter(content)?;
    let asins = parse_array_field(&fm, "asin");
    let brands = parse_aligned_array(&fm, "brand");
    let prices = parse_aligned_array(&fm, "price");
    let ratings = parse_aligned_array(&fm, "rating");
    let urls = parse_aligned_array(&fm, "url");
    let revieweds = parse_aligned_array(&fm, "reviewed");
    let votes = parse_aligned_array(&fm, "helpful_votes");
    let tones = parse_aligned_array(&fm, "tone");
    let date_str = scalar_field(&fm, "date");
    let parsed_date = parse_order_date(&date_str);

    let items_area = match find(re(r"##\s*📦\s*Items\r?\n([\s\S]*)$", ""), content) {
        Some(m) => m.group(content, 1).unwrap_or("").to_string(),
        None => content.to_string(),
    };
    let blocks: Vec<String> =
        js::split(re(r"\r?\n(?=###\s)", ""), &items_area).into_iter().filter(|b| js::test(re(r"^###\s", ""), b)).collect();

    let get = |v: &Vec<String>, i: usize| v.get(i).cloned().unwrap_or_default();
    let mut items = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let asin = get(&asins, i);
        let name = find(re(r"^###\s+(.+)$", "m"), block).map(|m| js::trim(m.group(block, 1).unwrap_or("")).to_string());
        let title = find(re(r"\*\*Title:\*\*\s*(.+)", ""), block).map(|m| js::trim(m.group(block, 1).unwrap_or("")).to_string());
        let review_text = find(re(r"\*\*Review:\*\*[ \t]*([\s\S]*?)(?=\r?\n- \[|\r?\n---|$)", ""), block)
            .map(|m| js::trim(m.group(block, 1).unwrap_or("")).to_string())
            .unwrap_or_default();
        let draft_rating = find(re(r"\*\*Rating:\*\*\s*([1-5])(?!\d)", ""), block).map(|m| m.group(block, 1).unwrap_or("").to_string());
        let image = if asin.is_empty() { Value::Null } else { image_for(&asin).map(Value::String).unwrap_or(Value::Null) };
        let hv = non_negative_int_str(votes.get(i).map(|s| s.as_str())).unwrap_or(0.0);
        let mut it = Map::new();
        it.insert("asin".into(), json!(asin));
        it.insert("name".into(), json!(name.unwrap_or_else(|| asin.clone())));
        it.insert("brand".into(), json!(get(&brands, i)));
        it.insert("price".into(), json!(get(&prices, i)));
        it.insert("helpfulVotes".into(), js::jnum(hv));
        it.insert("tone".into(), json!(get(&tones, i)));
        it.insert("rating".into(), json!(get(&ratings, i)));
        it.insert("draftRating".into(), json!(draft_rating.unwrap_or_default()));
        it.insert("url".into(), json!(get(&urls, i)));
        it.insert("image".into(), image);
        it.insert("reviewed".into(), json!(revieweds.get(i).map(|s| s == "true").unwrap_or(false)));
        it.insert("hasDraft".into(), json!(!review_text.is_empty()));
        it.insert("posted".into(), json!(is_posted_block(block)));
        it.insert("reviewTitle".into(), json!(title.unwrap_or_default()));
        it.insert("reviewText".into(), json!(review_text));
        items.push(Value::Object(it));
    }
    let mut o = Map::new();
    o.insert("orderId".into(), json!(scalar_field(&fm, "order_id")));
    o.insert("date".into(), json!(date_str));
    o.insert("total".into(), json!(scalar_field(&fm, "total")));
    o.insert("status".into(), json!(scalar_field(&fm, "status")));
    o.insert("shipTo".into(), json!(scalar_field(&fm, "ship_to")));
    o.insert("path".into(), json!(rel_path));
    o.insert("modified".into(), js::jnum(modified));
    o.insert("items".into(), Value::Array(items));
    Some(ParsedNote { order: Value::Object(o), sort_key: parsed_date.unwrap_or(modified) })
}

// ---------------------------------------------------------------------------
// Editing a note
// ---------------------------------------------------------------------------

pub struct ItemBlock {
    pub index: usize,
    pub start: usize,
    pub end: usize,
}

/// The item's section: its index in the asin array -> the i-th `### ` heading.
pub fn find_item_block(md: &str, asin: &str) -> Option<ItemBlock> {
    let fm = frontmatter(md)?;
    let asins = parse_array_field(&fm, "asin");
    let index = asins.iter().position(|a| a == asin)?;
    let headings: Vec<usize> = js::find_all(re(r"^### .+$", "gm"), md).iter().map(|m| m.start).collect();
    if index >= headings.len() {
        return None;
    }
    let start = headings[index];
    let end = if index + 1 < headings.len() { headings[index + 1] } else { md.len() };
    Some(ItemBlock { index, start, end })
}

/// Where the review text sits inside an item block: (start, end).
fn review_span(block: &str) -> Option<(usize, usize)> {
    let header_idx = block.find(REVIEW_HEADER)?;
    let mut search_start = header_idx + REVIEW_HEADER.len();
    if let Some(link_idx) = js::index_of(block, REVIEW_LINK, header_idx) {
        search_start = match js::index_of(block, "\n", link_idx) {
            Some(eol) => eol + 1,
            None => link_idx + REVIEW_LINK.len(),
        };
    }
    let posted_idx = js::index_of(block, "Posted to Amazon?", search_start)?;
    let end = match js::last_index_of(block, "- [", posted_idx) {
        Some(cb) if cb >= search_start => cb,
        _ => posted_idx,
    };
    Some((search_start, end))
}

/// The review text currently in an item block (between the link line and
/// the checkbox), trimmed. Used by re-sync to keep what the user wrote.
pub fn existing_review(block: &str) -> Option<String> {
    let (a, b) = review_span(block)?;
    if b < a {
        return Some(String::new());
    }
    Some(js::trim(&block[a..b]).to_string())
}

pub fn update_review_block(md: &str, asin: &str, title: &str, text: &str, rating: Option<f64>) -> Option<String> {
    let title = js::replace_str(re(r"\r?\n", "g"), &safe_review_text(title), true, " ");
    let text = safe_review_text(text);
    let loc = find_item_block(md, asin)?;
    let block = &md[loc.start..loc.end];
    let (search_start, end) = review_span(block)?;
    let rating_line = match rating {
        Some(r) => Some(format!("**Rating:** {}", normalize_rating(r))),
        None => {
            let seg = if end >= search_start { &block[search_start..end] } else { "" };
            find(re(r"^\*\*Rating:\*\*[^\n]*$", "m"), seg).map(|m| m.text(seg).to_string())
        }
    };
    let new_review = format!("\n{}**Title:** {}\n**Review:** {}\n", rating_line.map(|l| l + "\n").unwrap_or_default(), title, text);
    // block.slice(0, a) + new + block.slice(b): JS slices clamp, so b < a
    // repeats the overlap like JS does.
    let new_block = format!("{}{}{}", &block[..search_start], new_review, &block[end..]);
    Some(format!("{}{}{}", &md[..loc.start], new_block, &md[loc.end..]))
}

pub fn set_frontmatter_array_value(md: &str, field: &str, asin: &str, value: &str) -> Option<String> {
    let m = find(re(r"^---\r?\n([\s\S]*?)(\r?\n)---", ""), md)?;
    let fm = m.group(md, 1).unwrap_or("");
    let nl = m.group(md, 2).unwrap_or("\n");
    let asins = parse_array_field(fm, "asin");
    let index = asins.iter().position(|a| a == asin)?;
    let existing = parse_aligned_array(fm, field);
    let mut values: Vec<String> = (0..asins.len()).map(|i| existing.get(i).cloned().unwrap_or_default()).collect();
    values[index] = value.to_string();
    let line = format!("{}: [{}]", field, values.iter().map(|v| yaml_escape(v)).collect::<Vec<_>>().join(", "));
    let line_src = format!(r"^{}:.*$", field);
    let new_fm = if js::test(re(&line_src, "m"), fm) { replace_first(&line_src, "m", fm, &line) } else { format!("{}{}{}", fm, nl, line) };
    Some(format!("{}---{}{}{}---{}", &md[..m.start], nl, new_fm, nl, &md[m.end..]))
}

/// Flip the checkbox inside the first block (split on `---` lines) that
/// holds the ASIN and a review section. Returns (new content, changed).
pub fn set_checkbox(content: &str, asin: &str, posted: bool) -> (String, bool) {
    let mut parts = js::split(re(r"(\r?\n---\r?\n)", ""), content);
    let wanted = if posted { "x" } else { " " };
    let mut changed = false;
    for p in parts.iter_mut() {
        if !p.contains(asin) || !p.contains(REVIEW_HEADER) {
            continue;
        }
        let updated = replace_first(r"- \[[ xX]\](\s*Posted to Amazon\?)", "", p, &format!("- [{}]$1", wanted));
        if updated != *p {
            *p = updated;
            changed = true;
        }
        break;
    }
    (parts.concat(), changed)
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

pub fn guidance_for(err: &str) -> Value {
    let g = |message: &str, steps: &[&str]| json!({ "message": message, "steps": steps });
    if err.contains("login-required") {
        return g(
            "Your Amazon session expired.",
            &[
                "Run SETUP LOGIN to open the setup browser window.",
                "Log in to your Amazon account in that window.",
                "If you use Brave, turn Shields down for amazon.com.",
                "Close the window, then retry the sync.",
            ],
        );
    }
    if err.contains("no-browser") {
        return g(
            "No Chromium-based browser found.",
            &[
                "Install Brave, Edge or Chromium (Google Chrome no longer loads the extension in headless runs).",
                "Or set \"browserPath\" in config.json to your browser executable.",
            ],
        );
    }
    if err.starts_with("timeout") {
        return g("The sync browser didn't finish in time.", &[
            "This is usually a login or Shields issue — run SETUP LOGIN and check that the page loads.",
            "Google Chrome ignores the extension in headless runs (since Chrome 137): use Brave, Edge or Chromium, or point \"browserPath\" at one.",
        ]);
    }
    if err.contains("browser-stalled") {
        return g("The sync browser started but the extension never responded.", &[
            "Run SETUP LOGIN and confirm the orders page loads while logged in.",
            "If it persists, delete the browser-profile folder (see the SYNC tab for its path) and run SETUP LOGIN again.",
            "Make sure Brave Shields are down for amazon.com in the setup window.",
            "Google Chrome ignores the extension in headless runs (since Chrome 137): use Brave, Edge or Chromium, or point \"browserPath\" at one.",
        ]);
    }
    if err.contains("browser-exited") {
        return g(
            "The sync browser crashed or closed on its own.",
            &[
                "Run SETUP LOGIN and check the window opens and stays open.",
                "If it closes instantly, delete the browser-profile folder (see the SYNC tab for its path) and run SETUP LOGIN again.",
                "Make sure your browser is up to date.",
            ],
        );
    }
    if err.contains("gemini-limit") {
        return g(
            "Daily Gemini request budget reached.",
            &["Review drafts fall back to templates until midnight.", "Raise geminiDailyLimit if your AI Studio tier allows."],
        );
    }
    json!({ "message": err, "steps": [] })
}

pub fn csv_escape(s: &str) -> String {
    let mut s = s.to_string();
    if js::test(re(r"^\s*[=+\-@]", ""), &s) {
        s = format!("'{}", s);
    }
    if s.contains('"') || s.contains(',') || s.contains('\r') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Drop later items whose ASIN already appeared (first occurrence wins).
pub fn dedupe_items_by_asin(items: &[Value], order_id: Option<&Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let asin = to_str_or_empty(match item {
            Value::Object(m) => m.get("asin"),
            _ => None,
        });
        if asin.is_empty() {
            out.push(item.clone());
            continue;
        }
        if !seen.insert(asin.clone()) {
            crate::log!("[Dedupe] Dropped duplicate ASIN {} in order {}", asin, to_str(order_id));
            continue;
        }
        out.push(item.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_arrays() {
        let c = "brand: [\"Acme, Inc.\", \"\", \"Foo]Bar\"]\nasin: [\"A1\", \"B2\"]\n";
        assert_eq!(parse_aligned_array(c, "brand"), vec!["Acme, Inc.", "", "Foo]Bar"]);
        assert_eq!(parse_array_field(c, "asin"), vec!["A1", "B2"]);
        assert_eq!(yaml_escape("a\"b\\c\r\nd"), "\"a\\\"b\\\\c d\"");
    }

    #[test]
    fn prices() {
        assert_eq!(parse_price_str("$1,234.56"), Some(1234.56));
        assert_eq!(parse_price_str("1.234,56 €"), Some(1234.56));
        assert_eq!(parse_price_str("12,99"), Some(12.99));
        assert_eq!(parse_price_str("1,234"), Some(1234.0));
        assert_eq!(parse_price_str("free"), None);
    }

    #[test]
    fn seeds() {
        assert_eq!(seed_from_string(""), 2_166_136_261);
        assert_eq!(seed_from_string("a"), 0xe40c292c);
    }
}

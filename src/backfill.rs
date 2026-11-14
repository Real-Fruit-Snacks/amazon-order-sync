//! Importing Amazon "Request My Data" order-history CSV files as notes.

use crate::js;
use crate::jsdate::MONTH_NAMES;
use crate::notes;
use crate::store::Store;
use serde_json::{json, Value};

/// RFC-4180: quoted fields, embedded commas and newlines, doubled quotes.
/// Keys are the lowercased, trimmed header names.
/// One CSV row: (lowercased header, value) pairs in column order.
pub type Row = Vec<(String, String)>;

pub fn parse_csv(text: &str) -> Vec<Row> {
    let s = text.strip_prefix('\u{feff}').unwrap_or(text);
    let chars: Vec<char> = s.chars().collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let end_record = |field: &mut String, record: &mut Vec<String>, rows: &mut Vec<Vec<String>>| {
        record.push(std::mem::take(field));
        if record.len() > 1 || !record[0].is_empty() {
            rows.push(std::mem::take(record));
        } else {
            record.clear();
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if in_quotes {
            if ch == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    field.push('"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else if ch == '"' {
            in_quotes = true;
        } else if ch == ',' {
            record.push(std::mem::take(&mut field));
        } else if ch == '\n' {
            end_record(&mut field, &mut record, &mut rows);
        } else if ch == '\r' {
            if chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            end_record(&mut field, &mut record, &mut rows);
        } else {
            field.push(ch);
        }
        i += 1;
    }
    if !field.is_empty() || !record.is_empty() {
        end_record(&mut field, &mut record, &mut rows);
    }
    if rows.is_empty() {
        return Vec::new();
    }
    let headers: Vec<String> = rows[0].iter().map(|h| js::trim(h).to_lowercase()).collect();
    let mut out = Vec::new();
    for row in rows.iter().skip(1) {
        let mut obj: Vec<(String, String)> = Vec::new();
        for (c, h) in headers.iter().enumerate() {
            if h.is_empty() {
                continue;
            }
            let v = js::trim(row.get(c).map(String::as_str).unwrap_or("")).to_string();
            if let Some(e) = obj.iter_mut().find(|(k, _)| k == h) {
                e.1 = v;
            } else {
                obj.push((h.clone(), v));
            }
        }
        out.push(obj);
    }
    out
}

fn col<'a>(row: &'a [(String, String)], name: &str) -> Option<&'a str> {
    row.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// ISO (`2024-05-14`, optional time) or `MM/DD/YYYY` -> `Ordered on Month D, YYYY`.
pub fn parse_backfill_date(raw: &str) -> String {
    let s = js::trim(raw);
    if s.is_empty() {
        return "Ordered on Unknown".into();
    }
    let (year, month, day);
    if let Some(m) = js::find(js::re(r"^(\d{4})-(\d{1,2})-(\d{1,2})(?:[T ].*)?$", ""), s) {
        year = m.group(s, 1).unwrap().parse::<i64>().unwrap();
        month = m.group(s, 2).unwrap().parse::<i64>().unwrap();
        day = m.group(s, 3).unwrap().parse::<i64>().unwrap();
    } else if let Some(m) = js::find(js::re(r"^(\d{1,2})\/(\d{1,2})\/(\d{4})$", ""), s) {
        month = m.group(s, 1).unwrap().parse::<i64>().unwrap();
        day = m.group(s, 2).unwrap().parse::<i64>().unwrap();
        year = m.group(s, 3).unwrap().parse::<i64>().unwrap();
    } else {
        return "Ordered on Unknown".into();
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1900..=2999).contains(&year) {
        return "Ordered on Unknown".into();
    }
    if chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32).is_none() {
        return "Ordered on Unknown".into();
    }
    format!("Ordered on {} {}, {}", MONTH_NAMES[(month - 1) as usize], day, year)
}

fn format_backfill_price(unit: Option<&str>, qty: i64, currency: Option<&str>) -> String {
    let Some(u) = unit.and_then(notes::parse_price_str) else { return String::new() };
    let amount = js::to_fixed(u * qty as f64, 2);
    let cur = js::trim(currency.unwrap_or("")).to_uppercase();
    if cur.is_empty() || cur == "USD" {
        format!("${}", amount)
    } else {
        format!("{} {}", cur, amount)
    }
}

pub struct CsvFile {
    pub name: String,
    pub content: String,
}

pub fn import_files(store: &Store, files: &[CsvFile]) -> Value {
    let mut imported_orders = 0;
    let mut imported_items = 0;
    let mut skipped_existing = 0;
    let mut skipped_rows = 0;
    let mut errors: Vec<String> = Vec::new();

    for file in files {
        let rows = parse_csv(&file.content);
        let mut groups: Vec<(String, Vec<&Row>)> = Vec::new();
        for row in &rows {
            let order_id = col(row, "order id").unwrap_or("");
            let name = col(row, "product name").unwrap_or("");
            let asin = col(row, "asin").unwrap_or("");
            if order_id.is_empty() || name.is_empty() || asin.is_empty() {
                skipped_rows += 1;
                continue;
            }
            match groups.iter_mut().find(|(k, _)| k == order_id) {
                Some(g) => g.1.push(row),
                None => groups.push((order_id.to_string(), vec![row])),
            }
        }
        for (raw_id, order_rows) in groups {
            let order_id = notes::sanitize_id_str(&raw_id);
            if !notes::is_safe_id(&order_id) {
                skipped_rows += order_rows.len();
                errors.push(format!("{}: invalid order id \"{}\"", file.name, raw_id));
                continue;
            }
            if store.find_order_file(&order_id).is_some() {
                skipped_existing += 1;
                continue;
            }
            let first = |c: &str| -> String {
                for r in &order_rows {
                    if let Some(v) = col(r, c).filter(|v| !v.is_empty()) {
                        return v.to_string();
                    }
                }
                String::new()
            };
            let items: Vec<Value> = order_rows
                .iter()
                .map(|r| {
                    let q = js::parse_int10(col(r, "quantity").unwrap_or("undefined"));
                    let qty = if js::is_integer(q) && q > 0.0 { q as i64 } else { 1 };
                    json!({
                        "asin": col(r, "asin").unwrap_or(""),
                        "name": col(r, "product name").unwrap_or(""),
                        "brand": "",
                        "current_price": format_backfill_price(col(r, "unit price"), qty, col(r, "currency")),
                        "link": "",
                        "features": format!("- Quantity: {}\n- Source: Amazon data export", qty),
                        "image": "",
                    })
                })
                .collect();
            let ship_to = first("shipping address");
            let ship_to = js::trim(ship_to.split(',').next().unwrap_or("")).to_string();
            let status = {
                let s = first("order status");
                if s.is_empty() {
                    first("shipment status")
                } else {
                    s
                }
            };
            let n = items.len();
            let mut payload = json!({
                "order_id": raw_id,
                "date": parse_backfill_date(&first("order date")),
                "total": first("total owed"),
                "status": status,
                "ship_to": ship_to,
                "backfill": true,
                "items": items,
            });
            match store.create_or_update_order(&mut payload, false) {
                Ok(()) => {
                    imported_orders += 1;
                    imported_items += n;
                }
                Err(e) => errors.push(format!("{}: order {}: {}", file.name, raw_id, e)),
            }
        }
    }
    json!({
        "ok": true,
        "imported_orders": imported_orders,
        "imported_items": imported_items,
        "skipped_existing": skipped_existing,
        "skipped_rows": skipped_rows,
        "errors": errors,
    })
}

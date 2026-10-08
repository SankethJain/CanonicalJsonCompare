//! Helpers for MongoDB Extended JSON (canonical and relaxed).
//!
//! `mongoexport --jsonFormat=canonical` wraps every BSON type that plain JSON
//! cannot express in a single-key object such as `{"$oid": "..."}` or
//! `{"$date": {"$numberLong": "..."}}`. These helpers treat those wrappers as
//! single values so they can be compared and shown to people in a readable way.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{Map, Value};

/// Keys that mark an object as an Extended JSON value rather than a document.
const SCALAR_KEYS: &[&str] = &[
    "$oid",
    "$date",
    "$numberInt",
    "$numberLong",
    "$numberDouble",
    "$numberDecimal",
    "$binary",
    "$uuid",
    "$timestamp",
    "$regularExpression",
    "$regex",
    "$symbol",
    "$code",
    "$minKey",
    "$maxKey",
    "$undefined",
    "$dbPointer",
];

/// True when the object is an Extended JSON wrapper (e.g. `{"$oid": ".."}`).
pub fn is_ext_scalar(map: &Map<String, Value>) -> bool {
    !map.is_empty()
        && map.keys().all(|k| k.starts_with('$'))
        && map.keys().any(|k| SCALAR_KEYS.contains(&k.as_str()))
}

/// The BSON type of a value, as people would recognise it from MongoDB tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BType {
    Null,
    Bool,
    Int32,
    Int64,
    Double,
    Decimal128,
    /// A bare JSON number (relaxed format), type not specified.
    Number,
    String,
    ObjectId,
    Date,
    Binary,
    Timestamp,
    Regex,
    Object,
    Array,
    Other,
}

impl BType {
    pub fn name(self) -> &'static str {
        match self {
            BType::Null => "Null",
            BType::Bool => "Boolean",
            BType::Int32 => "Int32",
            BType::Int64 => "Int64",
            BType::Double => "Double",
            BType::Decimal128 => "Decimal128",
            BType::Number => "Number",
            BType::String => "String",
            BType::ObjectId => "ObjectId",
            BType::Date => "Date",
            BType::Binary => "Binary",
            BType::Timestamp => "Timestamp",
            BType::Regex => "Regex",
            BType::Object => "Object",
            BType::Array => "Array",
            BType::Other => "Other",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            BType::Int32 | BType::Int64 | BType::Double | BType::Decimal128 | BType::Number
        )
    }
}

pub fn btype(v: &Value) -> BType {
    match v {
        Value::Null => BType::Null,
        Value::Bool(_) => BType::Bool,
        Value::Number(_) => BType::Number,
        Value::String(_) => BType::String,
        Value::Array(_) => BType::Array,
        Value::Object(m) => {
            if !is_ext_scalar(m) {
                return BType::Object;
            }
            let key = m.keys().find(|k| SCALAR_KEYS.contains(&k.as_str()));
            match key.map(String::as_str) {
                Some("$oid") => BType::ObjectId,
                Some("$date") => BType::Date,
                Some("$numberInt") => BType::Int32,
                Some("$numberLong") => BType::Int64,
                Some("$numberDouble") => BType::Double,
                Some("$numberDecimal") => BType::Decimal128,
                Some("$binary") | Some("$uuid") => BType::Binary,
                Some("$timestamp") => BType::Timestamp,
                Some("$regularExpression") | Some("$regex") => BType::Regex,
                _ => BType::Other,
            }
        }
    }
}

/// Returns the textual number for numeric values (wrapped or bare).
pub fn number_text(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::Object(m) if m.len() == 1 => {
            for key in [
                "$numberInt",
                "$numberLong",
                "$numberDouble",
                "$numberDecimal",
            ] {
                if let Some(x) = m.get(key) {
                    return match x {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    };
                }
            }
            None
        }
        _ => None,
    }
}

/// Compares two textual numbers by value ("5" == "5.0" == "5E0").
pub fn numbers_equal(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if let (Ok(x), Ok(y)) = (a.parse::<i128>(), b.parse::<i128>()) {
        return x == y;
    }
    match (parse_f64(a), parse_f64(b)) {
        (Some(x), Some(y)) => x == y || (x.is_nan() && y.is_nan()),
        _ => false,
    }
}

fn parse_f64(s: &str) -> Option<f64> {
    match s {
        "Infinity" | "+Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        "NaN" | "-NaN" => Some(f64::NAN),
        _ => s.parse().ok(),
    }
}

/// Milliseconds since the Unix epoch for a BSON date value (`{"$date": ..}`).
pub fn date_millis(v: &Value) -> Option<i64> {
    let inner = v.as_object()?.get("$date")?;
    match inner {
        Value::Object(_) => number_text(inner)?.parse().ok(),
        Value::String(s) => parse_date_text(s),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// Like [`date_millis`] but also accepts dates stored as text or as epoch
/// numbers, which is common in collections that were not written carefully.
pub fn date_like_millis(v: &Value) -> Option<i64> {
    if let Some(ms) = date_millis(v) {
        return Some(ms);
    }
    match v {
        Value::String(s) => parse_date_text(s),
        _ => {
            let n: f64 = number_text(v)?.parse().ok()?;
            // Heuristic: values this large are milliseconds, smaller ones seconds.
            // Anything outside roughly 1971..2100 is not treated as a date.
            let ms = if n.abs() >= 1e11 { n } else { n * 1000.0 };
            (3.2e10..4.2e12).contains(&ms).then_some(ms as i64)
        }
    }
}

pub fn parse_date_text(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_millis());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(d.and_utc().timestamp_millis());
        }
    }
    if let Ok(d) = DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f %z") {
        return Some(d.timestamp_millis());
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc().timestamp_millis())
}

pub fn to_utc(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

/// `2024-03-15 10:42:07 UTC`
pub fn fmt_datetime(ms: i64) -> String {
    match to_utc(ms) {
        Some(d) => d.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms} ms"),
    }
}

/// `2024-03-15 10:42`
pub fn fmt_datetime_short(ms: Option<i64>) -> String {
    match ms.and_then(to_utc) {
        Some(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".into(),
    }
}

fn fmt_iso(ms: i64) -> String {
    match to_utc(ms) {
        Some(d) => d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => format!("{ms}"),
    }
}

/// Hex string of an ObjectId value, if the value is one.
pub fn oid_hex(v: &Value) -> Option<&str> {
    v.as_object()?.get("$oid")?.as_str()
}

/// Creation time embedded in an ObjectId (its first 4 bytes, in seconds).
pub fn oid_timestamp_millis(hex: &str) -> Option<i64> {
    let secs = u32::from_str_radix(hex.get(0..8)?, 16).ok()?;
    Some(secs as i64 * 1000)
}

/// Key used to pair up documents of both files. ObjectIds, strings and
/// numbers are keyed by their plain text so that the same `_id` matches even
/// when one file uses canonical and the other relaxed formatting.
pub fn id_key(v: &Value) -> String {
    if let Some(hex) = oid_hex(v) {
        return hex.to_ascii_lowercase();
    }
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    if let Some(n) = number_text(v) {
        return n;
    }
    display(v)
}

/// Looks up a (possibly dotted) field path such as `meta.createdAt`.
pub fn get_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = doc;
    for part in path.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    Some(cur)
}

/// Human friendly rendering of any value, e.g. `ObjectId("65f..")`,
/// `2024-03-15T10:42:07.000Z`, `42`, `"text"`, `{name: "x", qty: 2}`.
pub fn display(v: &Value) -> String {
    let mut out = String::new();
    write_display(v, &mut out);
    out
}

fn write_display(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_display(item, out);
            }
            out.push(']');
        }
        Value::Object(m) if is_ext_scalar(m) => out.push_str(&display_ext(v, m)),
        Value::Object(m) => {
            out.push('{');
            for (i, (k, item)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(": ");
                write_display(item, out);
            }
            out.push('}');
        }
    }
}

fn display_ext(v: &Value, m: &Map<String, Value>) -> String {
    if let Some(hex) = oid_hex(v) {
        return format!("ObjectId(\"{hex}\")");
    }
    if m.contains_key("$date") {
        return match date_millis(v) {
            Some(ms) => fmt_iso(ms),
            None => compact_json(v),
        };
    }
    if let Some(n) = number_text(v) {
        return n;
    }
    if let Some(b) = m.get("$binary") {
        let data = b
            .get("base64")
            .and_then(Value::as_str)
            .or_else(|| b.as_str())
            .unwrap_or("");
        let short: String = data.chars().take(24).collect();
        let ellipsis = if data.len() > 24 { "…" } else { "" };
        return format!("Binary(\"{short}{ellipsis}\")");
    }
    if let Some(u) = m.get("$uuid").and_then(Value::as_str) {
        return format!("UUID(\"{u}\")");
    }
    if let Some(t) = m.get("$timestamp") {
        let secs = t.get("t").map(|x| x.to_string()).unwrap_or_default();
        let inc = t.get("i").map(|x| x.to_string()).unwrap_or_default();
        return format!("Timestamp({secs}, {inc})");
    }
    if let Some(r) = m.get("$regularExpression") {
        let p = r.get("pattern").and_then(Value::as_str).unwrap_or("");
        let o = r.get("options").and_then(Value::as_str).unwrap_or("");
        return format!("/{p}/{o}");
    }
    if m.contains_key("$minKey") {
        return "MinKey".into();
    }
    if m.contains_key("$maxKey") {
        return "MaxKey".into();
    }
    if m.contains_key("$undefined") {
        return "undefined".into();
    }
    compact_json(v)
}

fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// Semantic equality for two values of the same non-numeric BSON type.
pub fn scalars_equal(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (date_millis(a), date_millis(b)) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (oid_hex(a), oid_hex(b)) {
        return x.eq_ignore_ascii_case(y);
    }
    a == b
}

/// Shortens text to `max` characters, adding an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognises_wrappers() {
        assert_eq!(
            btype(&json!({"$oid": "65f0a1b2c3d4e5f601234567"})),
            BType::ObjectId
        );
        assert_eq!(btype(&json!({"$numberInt": "5"})), BType::Int32);
        assert_eq!(btype(&json!({"$date": {"$numberLong": "0"}})), BType::Date);
        assert_eq!(btype(&json!({"a": 1})), BType::Object);
        assert_eq!(btype(&json!({"$ref": "x", "$id": 1})), BType::Object);
    }

    #[test]
    fn dates_in_both_formats() {
        let canonical = json!({"$date": {"$numberLong": "1710499327000"}});
        let relaxed = json!({"$date": "2024-03-15T10:42:07Z"});
        assert_eq!(date_millis(&canonical), Some(1710499327000));
        assert_eq!(date_millis(&relaxed), Some(1710499327000));
        assert!(scalars_equal(&canonical, &relaxed));
        assert_eq!(date_like_millis(&json!("2024-03-15")), Some(1710460800000));
    }

    #[test]
    fn ids_and_display() {
        let oid = json!({"$oid": "65F0A1B2C3D4E5F601234567"});
        assert_eq!(id_key(&oid), "65f0a1b2c3d4e5f601234567");
        assert_eq!(id_key(&json!({"$numberInt": "7"})), id_key(&json!(7)));
        assert_eq!(
            display(&json!({"a": {"$numberInt": "1"}, "b": "x"})),
            "{a: 1, b: \"x\"}"
        );
        assert_eq!(
            oid_timestamp_millis("65f0a1b2c3d4e5f601234567"),
            Some(0x65f0a1b2_i64 * 1000)
        );
    }

    #[test]
    fn number_equality() {
        assert!(numbers_equal("5", "5.0"));
        assert!(numbers_equal("9007199254740993", "9007199254740993"));
        assert!(!numbers_equal("9007199254740993", "9007199254740992"));
        assert!(!numbers_equal("1", "2"));
    }
}

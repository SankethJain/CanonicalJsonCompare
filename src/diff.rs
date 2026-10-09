//! Property by property comparison of two documents.

use serde_json::Value;

use crate::extjson::{self, BType};

/// Values longer than this are shortened when stored in a difference.
const MAX_VALUE_CHARS: usize = 4000;

#[derive(Debug, Clone)]
pub struct DiffOptions {
    /// Field names (`notes`) or dotted paths (`meta.syncedAt`) to skip.
    pub ignore: Vec<String>,
    /// Treat `5`, `5.0`, Int32 5 and Int64 5 as the same value.
    pub loose_numbers: bool,
    /// Compare lists as unordered collections.
    pub ignore_array_order: bool,
    /// Stop recording differences for one object after this many.
    pub max_diffs: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions {
            ignore: Vec::new(),
            loose_numbers: false,
            ignore_array_order: false,
            max_diffs: 500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffKind {
    /// Same field, different value.
    Changed,
    /// Same field, the value has a different type (e.g. Int32 vs String).
    TypeChanged,
    /// The field exists only in the source document.
    OnlyInSource,
    /// The field exists only in the destination document.
    OnlyInDest,
}

impl DiffKind {
    pub fn label(self) -> &'static str {
        match self {
            DiffKind::Changed => "Value changed",
            DiffKind::TypeChanged => "Type changed",
            DiffKind::OnlyInSource => "Missing in destination",
            DiffKind::OnlyInDest => "Extra in destination",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diff {
    /// Location of the property, e.g. `address.city` or `items[2].price`.
    pub path: String,
    pub kind: DiffKind,
    pub source: Option<String>,
    pub dest: Option<String>,
    pub source_type: Option<&'static str>,
    pub dest_type: Option<&'static str>,
}

/// `items[3].tags[0]` -> `items[].tags[]`, used to group differences by field.
pub fn generic_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_index = false;
    for c in path.chars() {
        match c {
            '[' => {
                in_index = true;
                out.push_str("[]");
            }
            ']' => in_index = false,
            _ if in_index => {}
            _ => out.push(c),
        }
    }
    out
}

fn rule_matches(rules: &[String], path: &str, key: Option<&str>) -> bool {
    let generic = generic_path(path);
    rules.iter().any(|rule| {
        if rule.contains('.') {
            generic == *rule
                || generic
                    .strip_prefix(rule.as_str())
                    .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
        } else {
            match key {
                Some(k) => k == rule,
                None => generic
                    .split('.')
                    .any(|seg| seg.trim_end_matches("[]") == rule),
            }
        }
    })
}

/// True when the ignore rules cover this path (or one of its parents).
pub fn path_ignored(opts: &DiffOptions, path: &str) -> bool {
    !opts.ignore.is_empty() && rule_matches(&opts.ignore, path, None)
}

pub struct DiffResult {
    pub diffs: Vec<Diff>,
    pub truncated: bool,
}

pub fn diff_documents(source: &Value, dest: &Value, opts: &DiffOptions) -> DiffResult {
    let mut w = Walker {
        opts,
        diffs: Vec::new(),
        truncated: false,
    };
    let mut path = String::new();
    w.walk(source, dest, &mut path);
    DiffResult {
        diffs: w.diffs,
        truncated: w.truncated,
    }
}

struct Walker<'a> {
    opts: &'a DiffOptions,
    diffs: Vec<Diff>,
    truncated: bool,
}

impl Walker<'_> {
    fn push(&mut self, path: &str, kind: DiffKind, a: Option<&Value>, b: Option<&Value>) {
        if self.diffs.len() >= self.opts.max_diffs {
            self.truncated = true;
            return;
        }
        let show = |v: &Value| extjson::truncate(&extjson::display(v), MAX_VALUE_CHARS);
        self.diffs.push(Diff {
            path: if path.is_empty() {
                "(document)".into()
            } else {
                path.to_string()
            },
            kind,
            source: a.map(show),
            dest: b.map(show),
            source_type: a.map(|v| extjson::btype(v).name()),
            dest_type: b.map(|v| extjson::btype(v).name()),
        });
    }

    fn is_ignored(&self, path: &str, key: &str) -> bool {
        !self.opts.ignore.is_empty() && rule_matches(&self.opts.ignore, path, Some(key))
    }

    fn walk(&mut self, a: &Value, b: &Value, path: &mut String) {
        let (ta, tb) = (extjson::btype(a), extjson::btype(b));
        match (a, b) {
            (Value::Object(ma), Value::Object(mb))
                if ta == BType::Object && tb == BType::Object =>
            {
                let base_len = path.len();
                for (key, va) in ma {
                    push_key(path, key);
                    if !self.is_ignored(path, key) {
                        match mb.get(key) {
                            Some(vb) => self.walk(va, vb, path),
                            None => self.push(path, DiffKind::OnlyInSource, Some(va), None),
                        }
                    }
                    path.truncate(base_len);
                }
                for (key, vb) in mb {
                    if ma.contains_key(key) {
                        continue;
                    }
                    push_key(path, key);
                    if !self.is_ignored(path, key) {
                        self.push(path, DiffKind::OnlyInDest, None, Some(vb));
                    }
                    path.truncate(base_len);
                }
            }
            (Value::Array(xa), Value::Array(xb)) => {
                if self.opts.ignore_array_order {
                    let mut sa: Vec<&Value> = xa.iter().collect();
                    let mut sb: Vec<&Value> = xb.iter().collect();
                    sa.sort_by_cached_key(|v| sort_key(v));
                    sb.sort_by_cached_key(|v| sort_key(v));
                    self.walk_list(&sa, &sb, path);
                } else {
                    let sa: Vec<&Value> = xa.iter().collect();
                    let sb: Vec<&Value> = xb.iter().collect();
                    self.walk_list(&sa, &sb, path);
                }
            }
            _ if ta.is_numeric() && tb.is_numeric() => {
                let (na, nb) = (
                    extjson::number_text(a).unwrap_or_default(),
                    extjson::number_text(b).unwrap_or_default(),
                );
                if !extjson::numbers_equal(&na, &nb) {
                    self.push(path, DiffKind::Changed, Some(a), Some(b));
                } else if ta != tb
                    && ta != BType::Number
                    && tb != BType::Number
                    && !self.opts.loose_numbers
                {
                    self.push(path, DiffKind::TypeChanged, Some(a), Some(b));
                }
            }
            _ if ta != tb => self.push(path, DiffKind::TypeChanged, Some(a), Some(b)),
            _ => {
                if !extjson::scalars_equal(a, b) {
                    self.push(path, DiffKind::Changed, Some(a), Some(b));
                }
            }
        }
    }

    fn walk_list(&mut self, a: &[&Value], b: &[&Value], path: &mut String) {
        let base_len = path.len();
        for i in 0..a.len().max(b.len()) {
            path.push_str(&format!("[{i}]"));
            match (a.get(i), b.get(i)) {
                (Some(x), Some(y)) => self.walk(x, y, path),
                (Some(x), None) => self.push(path, DiffKind::OnlyInSource, Some(x), None),
                (None, Some(y)) => self.push(path, DiffKind::OnlyInDest, None, Some(y)),
                (None, None) => {}
            }
            path.truncate(base_len);
        }
    }
}

fn push_key(path: &mut String, key: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(key);
}

/// Stable ordering key used when list order should be ignored. Numbers are
/// normalised so that `{"$numberInt":"1"}` and `1` sort together.
fn sort_key(v: &Value) -> String {
    match extjson::number_text(v) {
        Some(n) => format!("n:{n}"),
        None => extjson::display(v),
    }
}

/// Flattens a document into `(path, value)` pairs for the "all fields" view.
pub fn flatten(doc: &Value) -> Vec<(String, &Value)> {
    fn go<'a>(v: &'a Value, path: &mut String, out: &mut Vec<(String, &'a Value)>) {
        match v {
            Value::Object(m) if !extjson::is_ext_scalar(m) && !m.is_empty() => {
                let base = path.len();
                for (k, child) in m {
                    push_key(path, k);
                    go(child, path, out);
                    path.truncate(base);
                }
            }
            Value::Array(items) if !items.is_empty() => {
                let base = path.len();
                for (i, child) in items.iter().enumerate() {
                    path.push_str(&format!("[{i}]"));
                    go(child, path, out);
                    path.truncate(base);
                }
            }
            _ => out.push((path.clone(), v)),
        }
    }
    let mut out = Vec::new();
    go(doc, &mut String::new(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff(a: Value, b: Value, opts: &DiffOptions) -> Vec<Diff> {
        diff_documents(&a, &b, opts).diffs
    }

    #[test]
    fn identical_across_formats() {
        let canonical = json!({"_id": {"$oid": "65f0a1b2c3d4e5f601234567"}, "n": {"$numberInt": "5"}, "d": {"$date": {"$numberLong": "1710499327000"}}});
        let relaxed = json!({"d": {"$date": "2024-03-15T10:42:07Z"}, "n": 5, "_id": {"$oid": "65f0a1b2c3d4e5f601234567"}});
        assert!(diff(canonical, relaxed, &DiffOptions::default()).is_empty());
    }

    #[test]
    fn nested_changes() {
        let a = json!({"addr": {"city": "Oslo", "zip": "1"}, "tags": ["a", "b"], "gone": 1});
        let b = json!({"addr": {"city": "Bergen", "zip": "1"}, "tags": ["a"], "new": true});
        let d = diff(a, b, &DiffOptions::default());
        let summary: Vec<(String, DiffKind)> = d.iter().map(|d| (d.path.clone(), d.kind)).collect();
        assert_eq!(
            summary,
            vec![
                ("addr.city".to_string(), DiffKind::Changed),
                ("tags[1]".to_string(), DiffKind::OnlyInSource),
                ("gone".to_string(), DiffKind::OnlyInSource),
                ("new".to_string(), DiffKind::OnlyInDest),
            ]
        );
        assert_eq!(d[0].source.as_deref(), Some("\"Oslo\""));
    }

    #[test]
    fn number_types() {
        let a = json!({"n": {"$numberInt": "5"}});
        let b = json!({"n": {"$numberLong": "5"}});
        assert_eq!(
            diff(a.clone(), b.clone(), &DiffOptions::default())[0].kind,
            DiffKind::TypeChanged
        );
        let loose = DiffOptions {
            loose_numbers: true,
            ..Default::default()
        };
        assert!(diff(a, b, &loose).is_empty());
        let d = diff(json!({"n": "5"}), json!({"n": 5}), &DiffOptions::default());
        assert_eq!(d[0].kind, DiffKind::TypeChanged);
    }

    #[test]
    fn ignore_rules_and_array_order() {
        let opts = DiffOptions {
            ignore: vec!["__v".into(), "meta.syncedAt".into()],
            ignore_array_order: true,
            ..Default::default()
        };
        let a =
            json!({"__v": 1, "meta": {"syncedAt": 1, "x": 1}, "l": [1, 2, 3], "sub": [{"__v": 3}]});
        let b =
            json!({"__v": 2, "meta": {"syncedAt": 2, "x": 1}, "l": [3, 1, 2], "sub": [{"__v": 4}]});
        assert!(diff(a, b, &opts).is_empty());
    }

    #[test]
    fn generic_paths() {
        assert_eq!(generic_path("items[12].tags[0]"), "items[].tags[]");
    }
}

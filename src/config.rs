//! Remembers the user's last choices between runs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub last_source: String,
    pub last_dest: String,
    pub created_field: String,
    pub updated_field: String,
    pub ignore_fields: String,
    pub loose_numbers: bool,
    pub ignore_array_order: bool,
    pub last_browse_dir: Option<PathBuf>,
}

fn config_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("mongo-compare")
            .join("settings.json"),
    )
}

impl Settings {
    pub fn load() -> Settings {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best effort: failing to save settings must never interrupt the user.
    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Splits "a, b ,c" into ["a", "b", "c"].
pub fn parse_list(s: &str) -> Vec<String> {
    s.split([',', ';', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

/// Cleans up a path that was typed, pasted or dragged into the terminal:
/// surrounding quotes, `file://` URLs, `\ ` escapes and a leading `~`.
pub fn clean_path(raw: &str) -> PathBuf {
    let mut s = raw.trim().to_string();
    // PowerShell drag and drop produces `& 'C:\path\file.json'`.
    if let Some(rest) = s.strip_prefix("& ") {
        s = rest.trim().to_string();
    }
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            s = s[1..s.len() - 1].to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("file://") {
        s = percent_decode(rest);
        // file:///C:/x on Windows
        if cfg!(windows) && s.starts_with('/') && s.as_bytes().get(2) == Some(&b':') {
            s.remove(0);
        }
    }
    if !cfg!(windows) && s.contains('\\') {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
        }
        s = out;
    }
    if (s == "~" || s.starts_with("~/") || s.starts_with("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(s[1..].trim_start_matches(['/', '\\']));
    }
    PathBuf::from(s)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_dragged_paths() {
        assert_eq!(
            clean_path("  '/tmp/my file.json' "),
            PathBuf::from("/tmp/my file.json")
        );
        assert_eq!(clean_path("\"/tmp/a.json\""), PathBuf::from("/tmp/a.json"));
        assert_eq!(
            clean_path("file:///tmp/my%20file.json"),
            PathBuf::from("/tmp/my file.json")
        );
        if !cfg!(windows) {
            assert_eq!(
                clean_path("/tmp/my\\ file.json"),
                PathBuf::from("/tmp/my file.json")
            );
        }
        assert_eq!(
            parse_list("__v, meta.syncedAt;x"),
            vec!["__v", "meta.syncedAt", "x"]
        );
    }
}

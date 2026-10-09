//! Small text formatting helpers shared by the screens and the export.

/// 1234567 -> "1,234,567"
pub fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 1536 -> "1.5 KB"
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// "12.5%" of part in whole.
pub fn pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0%".into();
    }
    let p = part as f64 * 100.0 / whole as f64;
    if p > 0.0 && p < 0.1 {
        "<0.1%".into()
    } else {
        format!("{p:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(1234567), "1,234,567");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(pct(1, 3), "33.3%");
    }
}

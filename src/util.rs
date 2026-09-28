//! Small helpers shared by several modules.

/// Natural sort key: "R10" sorts after "R2". Digit runs compare as numbers,
/// text runs case-insensitively.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Chunk {
    Num(u64),
    Text(String),
}

pub fn natural_key(text: &str) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digits = false;
    for ch in text.chars() {
        let d = ch.is_ascii_digit();
        if !cur.is_empty() && d != digits {
            out.push(flush(&cur, digits));
            cur.clear();
        }
        digits = d;
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(flush(&cur, digits));
    }
    out
}

fn flush(cur: &str, digits: bool) -> Chunk {
    if digits {
        Chunk::Num(cur.parse().unwrap_or(u64::MAX))
    } else {
        Chunk::Text(cur.to_lowercase())
    }
}

/// Format millimetres compactly: 30 -> "30", 2.5 -> "2.5", 0.125 -> "0.125".
pub fn fmt_mm(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".to_string() } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["R10", "R2", "r1", "C1", "TP12", "TP3"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, vec!["C1", "r1", "R2", "R10", "TP3", "TP12"]);
    }

    #[test]
    fn mm() {
        assert_eq!(fmt_mm(30.0), "30");
        assert_eq!(fmt_mm(2.5), "2.5");
        assert_eq!(fmt_mm(0.0), "0");
    }
}

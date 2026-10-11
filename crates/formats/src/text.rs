//! Shared helpers for MUGEN's text formats (.air .def .cns .cmd).

/// Decode file bytes leniently: strip a UTF-8 BOM, replace invalid UTF-8.
/// Community files are often CP1252 or Shift-JIS; names may garble but
/// structure survives, which is all the parsers need.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Strip a `;` comment and surrounding whitespace.
pub fn strip_comment(line: &str) -> &str {
    line.split(';').next().unwrap_or("").trim()
}

/// MUGEN's `atoi`: optional sign, leading digits, trailing garbage ignored,
/// no digits means 0. Saturates at the i32 range.
pub fn atoi(s: &str) -> i32 {
    let s = s.trim();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i64 = 0;
    for b in digits.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        n = n * 10 + i64::from(b - b'0');
        if n > i32::MAX as i64 {
            return if neg { i32::MIN } else { i32::MAX };
        }
    }
    if neg { -(n as i32) } else { n as i32 }
}

/// Float parse that accepts what MUGEN accepts; `None` on garbage.
pub fn atof(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok()
}

/// `[Name rest]` → `("name", "rest")`, lowercased name. Name includes the
/// trailing space when there is a rest part, mirroring MUGEN's split.
pub fn section(line: &str) -> Option<(String, &str)> {
    let line = strip_comment(line);
    let inner = line.strip_prefix('[')?;
    let end = inner.find(']')?;
    let inner = &inner[..end];
    Some(match inner.find(' ') {
        Some(i) => (inner[..=i].to_ascii_lowercase(), inner[i + 1..].trim()),
        None => (inner.to_ascii_lowercase(), ""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atoi_like_mugen() {
        assert_eq!(atoi("42"), 42);
        assert_eq!(atoi(" -7x"), -7);
        assert_eq!(atoi("abc"), 0);
        assert_eq!(atoi("+3.9"), 3);
        assert_eq!(atoi("99999999999"), i32::MAX);
        assert_eq!(atoi("-99999999999"), i32::MIN);
    }
    #[test]
    fn section_split() {
        assert_eq!(
            section("[Begin Action 200] ; x"),
            Some(("begin ".into(), "Action 200"))
        );
        assert_eq!(section("[Data]"), Some(("data".into(), "")));
        assert_eq!(section("0,0, 0,0, 5"), None);
    }
}

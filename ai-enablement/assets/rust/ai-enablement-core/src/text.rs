//! Strings as JavaScript measures them.
//!
//! JS strings are UTF-16: every length, limit, clip and slice in the reference runtime (`maxLength`, `maxChars`,
//! the 4,000-character result cap, cyrb53) counts UTF-16 code units. These helpers do the same over Rust's UTF-8
//! strings. Where a JS cut would split a surrogate pair (an emoji, say), the cut moves to keep whole characters:
//! the end of a slice drops the split character, the start of a slice skips it.

use std::cmp::Ordering;

/// JS whitespace (`String.prototype.trim`, `\s` in a regular expression): WhiteSpace and LineTerminator. Unlike
/// `char::is_whitespace` it includes U+FEFF and excludes U+0085.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// JS `\s` as a regular-expression character class body (for the `regex` crate), without the brackets.
pub const JS_SPACE_CLASS: &str =
    r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

/// JS `trim()`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JS `trimStart()`.
pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_space)
}

/// JS `trimEnd()`.
pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

/// JS `s.length`: UTF-16 code units.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Compare as JS sorts strings by default (`Array.prototype.sort`): by UTF-16 code units.
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// The byte offset of UTF-16 index `idx` (clamped to the string). Inside a surrogate pair: before the character
/// (`up == false`) or after it (`up == true`).
fn byte_offset(s: &str, idx: usize, up: bool) -> usize {
    let mut units = 0;
    for (b, c) in s.char_indices() {
        if units >= idx {
            return b;
        }
        let next = units + c.len_utf16();
        if next > idx {
            return if up { b + c.len_utf8() } else { b };
        }
        units = next;
    }
    s.len()
}

/// JS `s.slice(start, end)`: UTF-16 indices; negative ones count from the end.
pub fn js_slice(s: &str, start: i64, end: Option<i64>) -> &str {
    let len = utf16_len(s) as i64;
    let norm = |i: i64| if i < 0 { (len + i).max(0) } else { i.min(len) };
    let from = norm(start);
    let to = end.map_or(len, norm);
    if to <= from {
        return "";
    }
    let a = byte_offset(s, from as usize, true);
    let b = byte_offset(s, to as usize, false);
    if b <= a {
        ""
    } else {
        &s[a..b]
    }
}

/// The first `n` UTF-16 code units (JS `s.slice(0, n)` for `n >= 0`).
pub fn js_head(s: &str, n: usize) -> &str {
    &s[..byte_offset(s, n, false)]
}

/// JS `n.toLocaleString('en-US')`: thousands separators, at most three decimals (rounded half away from zero).
pub fn locale_count(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let fixed = crate::json::to_fixed(n.abs(), 3);
    let (int_part, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let frac = frac.trim_end_matches('0');
    let mut grouped = String::new();
    for (k, c) in int_part.chars().enumerate() {
        if k > 0 && (int_part.len() - k) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let zero = int_part.chars().all(|c| c == '0') && frac.is_empty();
    let sign = if n < 0.0 && !zero { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// The first character upper-cased (JS `s.charAt(0).toUpperCase() + s.slice(1)`).
pub fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_by_utf16_units() {
        let s = "a🙂b";
        assert_eq!(utf16_len(s), 4);
        assert_eq!(js_slice(s, 0, Some(2)), "a");
        assert_eq!(js_slice(s, 0, Some(3)), "a🙂");
        assert_eq!(js_slice(s, 2, None), "b");
        assert_eq!(js_slice("abcdef", 0, Some(-2)), "abcd");
        assert_eq!(js_slice("abcdef", -2, None), "ef");
        assert_eq!(js_slice("abc", 2, Some(1)), "");
    }

    #[test]
    fn counts_like_en_us() {
        assert_eq!(locale_count(1500.0), "1,500");
        assert_eq!(locale_count(999.0), "999");
        assert_eq!(locale_count(1234567.0), "1,234,567");
        assert_eq!(locale_count(-1234.5678), "-1,234.568");
        assert_eq!(locale_count(0.0), "0");
    }

    #[test]
    fn js_whitespace() {
        assert_eq!(js_trim("\u{FEFF} x \u{3000}"), "x");
        assert_eq!(js_trim("\u{85}x"), "\u{85}x");
    }
}

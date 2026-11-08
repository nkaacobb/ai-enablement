//! Time as the JS runtime writes it: milliseconds since the epoch, and ISO 8601 strings in UTC
//! (`Date.prototype.toISOString`). No time-zone database: every time here is UTC.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch (JS `Date.now()`).
pub fn now_millis() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    }
}

/// `new Date().toISOString()`.
pub fn iso_now() -> String {
    iso_from_millis(now_millis()).unwrap_or_default()
}

const MAX_TIME: f64 = 8.64e15;

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// `new Date(ms).toISOString()`, or None outside JS's date range (an invalid date).
pub fn iso_from_millis(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > MAX_TIME {
        return None;
    }
    let ms = ms.trunc() as i64;
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let (h, min, s, milli) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    let year = if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y < 0 {
        format!("-{:06}", -y)
    } else {
        format!("+{y:06}")
    };
    Some(format!("{year}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{milli:03}Z"))
}

/// `Date.parse` for the ISO 8601 forms JS defines (`2026-01-31`, `2026-01-31T10:00:00.000Z`, `…+02:00`), as
/// milliseconds; None for anything else. A date-time without an offset is read as UTC (JS reads it as local time).
pub fn parse_date(s: &str) -> Option<f64> {
    let b = s.trim().as_bytes();
    let mut i = 0;
    let num = |i: &mut usize, n: usize| -> Option<i64> {
        let part = b.get(*i..*i + n)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    let year = match b.first()? {
        b'+' | b'-' => {
            let neg = b[0] == b'-';
            i = 1;
            let y = num(&mut i, 6)?;
            if neg {
                -y
            } else {
                y
            }
        }
        _ => num(&mut i, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = num(&mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = num(&mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut h, mut min, mut sec, mut milli) = (0, 0, 0, 0);
    let mut offset = 0;
    if matches!(b.get(i), Some(b'T' | b't' | b' ')) {
        i += 1;
        h = num(&mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        min = num(&mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
            sec = num(&mut i, 2)?;
            if b.get(i) == Some(&b'.') {
                i += 1;
                let start = i;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let frac = std::str::from_utf8(&b[start..i]).ok()?;
                milli = format!("{frac:0<3}")[..3].parse().ok()?;
            }
        }
        match b.get(i) {
            Some(b'Z' | b'z') => i += 1,
            Some(c @ (b'+' | b'-')) => {
                let sign = if *c == b'-' { -1 } else { 1 };
                i += 1;
                let oh = num(&mut i, 2)?;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                let om = num(&mut i, 2)?;
                offset = sign * (oh * 60 + om);
            }
            _ => {}
        }
        if h > 24 || min > 59 || sec > 59 || (h == 24 && (min, sec, milli) != (0, 0, 0)) {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let ms = ((days * 24 + h) * 60 + min - offset) * 60_000 + sec * 1000 + milli;
    let ms = ms as f64;
    (ms.abs() <= MAX_TIME).then_some(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        let ms = parse_date("2026-06-01T00:00:00.000Z").unwrap();
        assert_eq!(ms, 1_780_272_000_000.0);
        assert_eq!(iso_from_millis(ms).unwrap(), "2026-06-01T00:00:00.000Z");
        assert_eq!(iso_from_millis(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert_eq!(parse_date("2026-01-02T03:04:05.5+01:00").unwrap(), parse_date("2026-01-02T02:04:05.500Z").unwrap());
        assert_eq!(parse_date("2026-01-01").unwrap(), parse_date("2026-01-01T00:00Z").unwrap());
        assert!(parse_date("yesterday").is_none());
        assert!(parse_date("").is_none());
    }
}

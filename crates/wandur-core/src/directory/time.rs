//! Timestamps as the directory sends them (RFC 3339, such as `2026-09-14T20:00:00+00:00` or
//! `2026-09-14T20:00:00.123Z`), turned into seconds since 1970. No time zone database is needed:
//! every value carries its offset.
use crate::l10n::{S, t, tf};

/// Seconds since 1970 (UTC) for an RFC 3339 timestamp, or `None` if it is not one.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let t = text.trim().as_bytes();
    if t.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let s = std::str::from_utf8(t.get(range)?).ok()?;
        if !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if t[4] != b'-'
        || t[7] != b'-'
        || !(t[10] == b'T' || t[10] == b't' || t[10] == b' ')
        || t[13] != b':'
        || t[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut i = 19;
    if t.get(i) == Some(&b'.') {
        i += 1;
        while t.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match t.get(i) {
        Some(b'Z' | b'z') if i + 1 == t.len() => 0,
        Some(sign @ (b'+' | b'-')) if t.len() == i + 6 && t[i + 3] == b':' => {
            let h = num(i + 1..i + 3)?;
            let m = num(i + 4..i + 6)?;
            let o = h * 3600 + m * 60;
            if *sign == b'+' { o } else { -o }
        }
        None => 0,
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The calendar date (year, month, day) of a day number since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The year of a timestamp (UTC).
pub fn year_of(secs: i64) -> i64 {
    civil_from_days(secs.div_euclid(86_400)).0
}

/// `YYYY-MM-DD` (UTC) of a timestamp.
pub fn date_text(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// A short relative time, as the C# client words it: just now, minutes, hours, days, then the date.
pub fn ago(then: i64, now: i64) -> String {
    let age = now - then;
    if age < 60 {
        t(S::JustNow).into()
    } else if age < 3600 {
        tf(S::MinutesAgo, &[&(age / 60)])
    } else if age < 86_400 {
        tf(S::HoursAgo, &[&(age / 3600)])
    } else if age < 60 * 86_400 {
        let d = age / 86_400;
        tf(if d == 1 { S::DaysAgoOne } else { S::DaysAgo }, &[&d])
    } else {
        date_text(then)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_offsets_fractions_and_zulu() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_rfc3339("2026-09-14T20:00:00+00:00"), Some(1_789_416_000));
        assert_eq!(parse_rfc3339("2026-09-14T20:00:00.1234567Z"), Some(1_789_416_000));
        assert_eq!(parse_rfc3339("2026-09-14T15:00:00-05:00"), Some(1_789_416_000));
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("yesterday"), None);
        assert_eq!(parse_rfc3339(""), None);
    }

    #[test]
    fn civil_round_trip_and_ago() {
        for days in [-1000, 0, 1, 59, 365, 20_000, 30_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(year_of(1_789_416_000), 2026);
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(ago(0, 30), "just now");
        assert_eq!(ago(0, 120), "2 min ago");
        assert_eq!(ago(0, 3600), "1 h ago");
        assert_eq!(ago(0, 86_400), "1 day ago");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
        assert_eq!(ago(0, 90 * 86_400), "1970-01-01");
    }
}

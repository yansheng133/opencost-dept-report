//! UTC timestamps without a time-zone database.
//!
//! Everything here is UTC on purpose. The billing day is the UTC day, the
//! container runs in UTC, and the only conversion to local time happens in
//! the browser. Mixing a local offset into gap reports once produced an
//! 8-hour "gap" that was really a time-zone bug.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, now.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date of a day number from [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (days_from_civil(ny, nm, 1) - days_from_civil(y, m, 1)) as u32
}

fn digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `YYYY-MM-DD` → (year, month, day), validated.
pub fn parse_day(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y = digits(&s[0..4])?;
    let m = digits(&s[5..7])? as u32;
    let d = digits(&s[8..10])? as u32;
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some((y, m, d))
}

/// An OpenCost or RFC 3339 UTC timestamp → epoch seconds.
///
/// Accepts `Z`, `+0000` and `+00:00`, and drops fractional seconds. When a
/// window has no data at all OpenCost returns no start; that is an error
/// here, not a panic, so every caller handles "no data" the same way.
pub fn parse_iso(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("no timestamp (the window may have no data at all)".into());
    }
    let t = t
        .strip_suffix('Z')
        .or_else(|| t.strip_suffix("+0000"))
        .or_else(|| t.strip_suffix("+00:00"))
        .ok_or_else(|| format!("not a UTC timestamp: {s}"))?;
    let t = t.split('.').next().unwrap_or(t);
    let bad = || format!("not a timestamp: {s}");
    let (date, clock) = t.split_once('T').ok_or_else(bad)?;
    let (y, m, d) = parse_day(date).ok_or_else(bad)?;
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.len() != 2) {
        return Err(bad());
    }
    let hh = digits(parts[0]).ok_or_else(bad)?;
    let mm = digits(parts[1]).ok_or_else(bad)?;
    let ss = digits(parts[2]).ok_or_else(bad)?;
    if hh > 23 || mm > 59 || ss > 60 {
        return Err(bad());
    }
    Ok(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Epoch seconds → `YYYY-MM-DDTHH:MM:SSZ`.
pub fn fmt_iso(epoch: f64) -> String {
    let secs = epoch.floor() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Epoch seconds → `YYYY-MM-DD` (UTC).
pub fn fmt_day(epoch: f64) -> String {
    fmt_iso(epoch)[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for s in [
            "1970-01-01T00:00:00Z",
            "2026-09-24T05:00:00Z",
            "2024-02-29T23:59:59Z",
            "2000-03-01T12:34:56Z",
        ] {
            assert_eq!(fmt_iso(parse_iso(s).unwrap() as f64), s);
        }
        assert_eq!(
            parse_iso("2026-09-24T05:00:00+0000").unwrap(),
            parse_iso("2026-09-24T05:00:00Z").unwrap()
        );
        assert_eq!(
            parse_iso("2026-09-24T05:00:00.123456Z").unwrap(),
            parse_iso("2026-09-24T05:00:00+00:00").unwrap()
        );
        assert_eq!(parse_iso("2026-09-24T00:00:00Z").unwrap(), 1_790_208_000);
    }

    #[test]
    fn rejects() {
        assert!(parse_iso("").is_err());
        assert!(parse_iso("2026-09-24T05:00:00+0800").is_err());
        assert!(parse_iso("2026-02-30T00:00:00Z").is_err());
        assert!(parse_iso("2026-09-24 05:00:00Z").is_err());
        assert!(parse_day("2026-9-24").is_none());
        assert!(parse_day("2026-13-01").is_none());
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
    }
}

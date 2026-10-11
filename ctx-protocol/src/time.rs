//! Time stamps without a date library: Unix milliseconds in, ISO 8601 text out.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 for a civil date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// `2026-10-10T17:52:01Z` for a Unix time in milliseconds.
pub fn iso_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-10-10 18:30` in a time zone `offset_seconds` east of UTC.
pub fn local_minutes(ms: i64, offset_seconds: i64) -> String {
    let secs = ms.div_euclid(1000) + offset_seconds;
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// `18:30` in a time zone `offset_seconds` east of UTC.
pub fn local_clock(ms: i64, offset_seconds: i64) -> String {
    let rem = (ms.div_euclid(1000) + offset_seconds).rem_euclid(86_400);
    format!("{:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// Parse `2026-10-10T17:52:01Z` (or a plain `2026-10-10`, taken as midnight UTC) back to ms.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t.trim_end_matches('Z'))),
        None => (s, None),
    };
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d) * 86_400;
    if let Some(t) = time {
        let mut tp = t.split(':');
        let h: i64 = tp.next()?.parse().ok()?;
        let mi: i64 = tp.next().unwrap_or("0").parse().ok()?;
        let se: f64 = tp.next().unwrap_or("0").parse().ok()?;
        secs += h * 3600 + mi * 60 + se as i64;
    }
    Some(secs * 1000)
}

/// `2.4 seconds`, `3 minutes 5 seconds`, `1 hour 2 minutes`: whole words, as the spec asks.
pub fn duration_words(seconds: f64) -> String {
    if seconds < 10.0 {
        let rounded = (seconds * 10.0).round() / 10.0;
        let unit = if rounded == 1.0 { "second" } else { "seconds" };
        return format!("{rounded} {unit}");
    }
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    let plural = |n: u64, w: &str| if n == 1 { format!("1 {w}") } else { format!("{n} {w}s") };
    if h > 0 {
        if m > 0 { format!("{} {}", plural(h, "hour"), plural(m, "minute")) } else { plural(h, "hour") }
    } else if m > 0 {
        if s > 0 { format!("{} {}", plural(m, "minute"), plural(s, "second")) } else { plural(m, "minute") }
    } else {
        plural(s, "second")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        let ms = 1_791_654_721_000; // 2026-10-10T17:52:01Z
        assert_eq!(iso_utc(ms), "2026-10-10T17:52:01Z");
        assert_eq!(parse_iso("2026-10-10T17:52:01Z"), Some(ms));
        assert_eq!(local_minutes(ms, 3600), "2026-10-10 18:52");
        assert_eq!(parse_iso("2026-10-10"), Some(days_from_civil(2026, 10, 10) * 86_400_000));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn durations_are_words() {
        assert_eq!(duration_words(2.43), "2.4 seconds");
        assert_eq!(duration_words(1.0), "1 second");
        assert_eq!(duration_words(65.0), "1 minute 5 seconds");
        assert_eq!(duration_words(7260.0), "2 hours 1 minute");
    }
}

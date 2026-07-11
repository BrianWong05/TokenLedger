// Parse "YYYY-MM-DDTHH:MM:SS(.fff)?Z" (always UTC) to epoch seconds.
// Howard Hinnant's days-from-civil algorithm; avoids a chrono dependency.
pub fn iso_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;

    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe - 719468; // days since 1970-01-01
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

// Inverse of iso_to_epoch (civil-from-days). Needed to stamp last_refresh
// when persisting refreshed Codex tokens.
pub fn epoch_to_iso(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, secs / 3600, (secs % 3600) / 60, secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_iso_round_trips() {
        let iso = "2026-06-01T10:00:00Z";
        assert_eq!(epoch_to_iso(iso_to_epoch(iso).unwrap()), iso);
        assert_eq!(epoch_to_iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn parses_basic_utc_timestamp() {
        assert_eq!(iso_to_epoch("2026-06-01T10:00:00.000Z"), Some(1780308000));
    }

    #[test]
    fn parses_without_fractional_seconds() {
        assert_eq!(iso_to_epoch("2026-06-01T10:00:00Z"), Some(1780308000));
    }

    #[test]
    fn parses_epoch_zero() {
        assert_eq!(iso_to_epoch("1970-01-01T00:00:00.000Z"), Some(0));
    }

    #[test]
    fn rejects_too_short_string() {
        assert_eq!(iso_to_epoch("2026-06-01"), None);
    }
}

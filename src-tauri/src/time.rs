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

#[cfg(test)]
mod tests {
    use super::*;

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

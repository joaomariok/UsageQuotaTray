//! Minimal UTC formatting so the spike needs no date crate.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Claude Code stores some times as Unix milliseconds; accept seconds or milliseconds.
pub fn epoch_to_secs(n: i64) -> i64 {
    if n.abs() >= 100_000_000_000 {
        n / 1000
    } else {
        n
    }
}

/// `YYYY-MM-DD HH:MM:SS UTC` for Unix seconds, using Howard Hinnant's civil-from-days algorithm.
pub fn format_utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// "in 2 h 5 min" or "3 min ago".
pub fn relative(target_secs: i64, now_secs: i64) -> String {
    let delta = target_secs - now_secs;
    let minutes = delta.abs() / 60;
    let span = if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    };
    if delta >= 0 {
        format!("in {span}")
    } else {
        format!("{span} ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(1_767_225_600), "2026-01-01 00:00:00 UTC");
        assert_eq!(format_utc(1_709_208_000), "2024-02-29 12:00:00 UTC");
        assert_eq!(format_utc(-1), "1969-12-31 23:59:59 UTC");
    }

    #[test]
    fn accepts_seconds_and_milliseconds() {
        assert_eq!(epoch_to_secs(1_767_225_600_000), 1_767_225_600);
        assert_eq!(epoch_to_secs(1_767_225_600), 1_767_225_600);
    }

    #[test]
    fn describes_relative_times() {
        assert_eq!(relative(1_000 + 125 * 60, 1_000), "in 2 h 5 min");
        assert_eq!(relative(1_000, 1_000 + 3 * 60 + 59), "3 min ago");
        assert_eq!(relative(1_000, 1_000), "in 0 min");
    }
}

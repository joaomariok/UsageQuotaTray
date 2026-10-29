//! Time without a date crate: Unix-second timestamps, civil-date conversion, and the
//! clock abstraction the engine reads time from (with a fake for tests).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// A UTC civil date and time. Month and day are 1-based.
    pub fn from_utc(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        let days = days_from_civil(year, month, day);
        Self(days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
    }

    /// The civil date and time of this instant shifted by `offset_secs` (for local time).
    pub fn civil(self, offset_secs: i64) -> Civil {
        let secs = self.0 + offset_secs;
        let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        Civil {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    pub fn plus_secs(self, secs: i64) -> Self {
        Self(self.0 + secs)
    }
}

/// A broken-down date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (m, d) = (i64::from(month), i64::from(day));
    let y = if m <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date for days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Where the engine gets the current time from.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// The real wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
        )
    }
}

/// A clock tests move by hand. Clones share the same time.
#[derive(Clone)]
pub struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    pub fn new(start: Timestamp) -> Self {
        Self(Arc::new(AtomicI64::new(start.0)))
    }

    pub fn advance(&self, secs: i64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }

    pub fn set(&self, to: Timestamp) {
        self.0.store(to.0, Ordering::SeqCst);
    }

    pub fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        FakeClock::now(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_known_instants_both_ways() {
        assert_eq!(Timestamp::from_utc(1970, 1, 1, 0, 0, 0), Timestamp(0));
        assert_eq!(
            Timestamp::from_utc(2026, 1, 1, 0, 0, 0),
            Timestamp(1_767_225_600)
        );
        assert_eq!(
            Timestamp::from_utc(2024, 2, 29, 12, 0, 0),
            Timestamp(1_709_208_000)
        );
        let c = Timestamp(-1).civil(0);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.minute, c.second),
            (1969, 12, 31, 23, 59, 59)
        );
    }

    #[test]
    fn applies_a_local_offset() {
        let c = Timestamp::from_utc(2026, 10, 29, 23, 30, 0).civil(3600);
        assert_eq!((c.month, c.day, c.hour, c.minute), (10, 30, 0, 30));
    }
}

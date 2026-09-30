//! Wall-clock helpers. Docker prints times as RFC 3339 in UTC and files carry `SystemTime`; the
//! app compares them ("did the configuration change after the server started?") as whole seconds
//! since the Unix epoch.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch, now.
pub fn now() -> u64 {
    epoch_of(SystemTime::now()).unwrap_or(0)
}

/// Seconds since the Unix epoch, or `None` for a time before it.
pub fn epoch_of(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian); negative before it.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since the Unix epoch for a UTC date and time; `None` when a field is out of range or the
/// time is before the epoch.
pub fn epoch_from_civil(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Option<u64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }
    u64::try_from(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

/// Reads a UTC time such as `2026-09-29T08:21:12.123456789Z` (the fraction is optional). Anything
/// else, including a time zone other than `Z` and Docker's "never started" year 1, is `None`.
pub fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (year, month, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let whole = time.split('.').next()?;
    let mut t = whole.split(':');
    let (h, m, s): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    if t.next().is_some() || h > 23 || m > 59 || s > 60 {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + s;
    u64::try_from(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_times_become_epoch_seconds() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        // The fraction is dropped, not rounded.
        assert_eq!(parse_rfc3339_utc("2026-09-29T08:21:12.987654321Z\n"), Some(1_790_670_072));
        // A leap day and the day after it.
        assert_eq!(parse_rfc3339_utc("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(parse_rfc3339_utc("2024-03-01T00:00:00Z"), Some(1_709_251_200));
    }

    #[test]
    fn a_civil_time_gives_the_same_seconds_as_the_text_form() {
        assert_eq!(epoch_from_civil(2026, 9, 29, 8, 21, 12), parse_rfc3339_utc("2026-09-29T08:21:12Z"));
        assert_eq!(epoch_from_civil(1970, 1, 1, 0, 0, 0), Some(0));
        assert_eq!(epoch_from_civil(2026, 13, 1, 0, 0, 0), None);
        assert_eq!(epoch_from_civil(2026, 9, 29, 24, 0, 0), None);
        assert_eq!(epoch_from_civil(1969, 12, 31, 23, 59, 59), None);
    }

    #[test]
    fn a_container_that_never_started_and_garbage_are_not_times() {
        assert_eq!(parse_rfc3339_utc("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_utc(""), None);
        assert_eq!(parse_rfc3339_utc("yesterday"), None);
        assert_eq!(parse_rfc3339_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_utc("2026-09-29T08:21:12+02:00"), None);
    }

    #[test]
    fn now_is_after_the_time_this_was_written() {
        assert!(now() > 1_790_000_000);
    }
}

//! Times as lazyrunner shows and keeps them: the local time, in the formats below; the state files keep RFC 3339
//! with the offset.

use jiff::fmt::temporal::Pieces;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};

/// A time of day: `15:30:07`.
pub const CLOCK: &str = "%H:%M:%S";
/// A day: `2026-11-19`.
pub const DAY: &str = "%Y-%m-%d";
/// A day and its time: `2026-11-19 15:30:07`.
pub const DAY_CLOCK: &str = "%Y-%m-%d %H:%M:%S";
/// A file name's stamp: `20261119-153007`.
pub const STAMP: &str = "%Y%m%d-%H%M%S";
/// RFC 3339 with the offset, to the second: `2026-11-19T15:30:07+01:00`.
pub const RFC_3339_SECONDS: &str = "%Y-%m-%dT%H:%M:%S%:z";
/// RFC 3339 with the offset and the fraction of a second: `2026-11-19T15:30:07.123+01:00`.
pub const RFC_3339: &str = "%Y-%m-%dT%H:%M:%S%.f%:z";

/// `at` in RFC 3339 with its offset, as the state files keep times.
pub fn rfc3339(at: &ZonedDateTime) -> String {
    at.strftime(RFC_3339).to_string()
}

/// Now in RFC 3339 with the local offset.
pub fn now_rfc3339() -> String {
    rfc3339(&ZonedDateTime::now())
}

/// An RFC 3339 time in the local time zone; none when it doesn't read.
pub fn local(text: &str) -> Option<ZonedDateTime> {
    Some(text.parse::<Timestamp>().ok()?.to_zoned_datetime(TimeZone::system()))
}

/// An RFC 3339 time at the offset it was written with; none when it doesn't read.
pub fn as_written(text: &str) -> Option<ZonedDateTime> {
    let offset = Pieces::parse(text).ok()?.to_numeric_offset()?;
    Some(text.parse::<Timestamp>().ok()?.to_zoned_datetime(TimeZone::fixed(offset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_written_by_older_versions_read_at_their_offset_and_write_back_alike() {
        let text = "2026-11-19T15:30:00.123+01:00";
        let at = as_written(text).unwrap();
        assert_eq!(at.strftime("%d %b %H:%M").to_string(), "19 Nov 15:30");
        assert_eq!(rfc3339(&at), text);
        let whole = as_written("2026-11-19T15:30:00+01:00").unwrap();
        assert_eq!(
            (rfc3339(&whole), whole.strftime(RFC_3339_SECONDS).to_string()),
            ("2026-11-19T15:30:00+01:00".to_string(), "2026-11-19T15:30:00+01:00".to_string())
        );
        assert_eq!(local(text).map(|at| at.timestamp()), Some(at.timestamp()));
        assert!(local("yesterday").is_none() && as_written("2026-11-19 15:30").is_none());
    }

    #[test]
    fn now_reads_back_as_the_same_instant() {
        let now = ZonedDateTime::now();
        assert_eq!(local(&rfc3339(&now)).map(|at| at.timestamp()), Some(now.timestamp()));
        assert!(local(&now_rfc3339()).is_some());
    }
}

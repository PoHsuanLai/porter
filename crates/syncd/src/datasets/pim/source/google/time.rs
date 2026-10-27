//! Dates and times as Google writes them (RFC 3339, or a bare `YYYY-MM-DD`) and as iCalendar
//! and vCard want them. Pure; chrono reads them.

use chrono::{DateTime, Datelike, Duration, NaiveDateTime, Timelike};

/// A parsed RFC 3339 time: the wall-clock fields as written and the offset they were written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// The wall clock as written, to the second (a fraction and a leap second are dropped).
    local: NaiveDateTime,
    /// East of UTC, in seconds.
    offset: i32,
}

impl Stamp {
    /// Reads `2026-10-06T10:00:00-07:00`, with an optional fraction and `Z` for a zero offset.
    /// A time with no offset at all is read as UTC, and an offset may be written `-0700`.
    pub fn parse(text: &str) -> Option<Self> {
        let (local, offset) = match DateTime::parse_from_rfc3339(text)
            .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z"))
        {
            Ok(at) => (at.naive_local(), at.offset().local_minus_utc()),
            Err(_) => (
                NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()?,
                0,
            ),
        };
        (local.year() >= 1).then_some(())?;
        Some(Self {
            local: local.with_nanosecond(0)?,
            offset,
        })
    }

    /// `20261006T100000`: the wall-clock time as written, for a `TZID=` property.
    pub fn local(&self) -> String {
        written(&self.local)
    }

    /// `20261006T170000Z`: the same instant in UTC.
    pub fn utc(&self) -> String {
        let at = self.local - Duration::seconds(i64::from(self.offset));
        format!("{}Z", written(&at))
    }
}

/// `20261006T100000` of `at`.
fn written(at: &NaiveDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_read_with_its_offset_and_written_local_and_in_utc() {
        const CASES: &[(&str, &str, &str)] = &[
            (
                "2026-10-06T10:00:00Z",
                "20261006T100000",
                "20261006T100000Z",
            ),
            (
                "2026-10-06T10:00:00.000Z",
                "20261006T100000",
                "20261006T100000Z",
            ),
            (
                "2026-10-06T10:00:00-07:00",
                "20261006T100000",
                "20261006T170000Z",
            ),
            (
                "2026-10-06T23:30:00-07:00",
                "20261006T233000",
                "20261007T063000Z",
            ),
            (
                "2026-10-06T01:30:00+05:30",
                "20261006T013000",
                "20261005T200000Z",
            ),
            (
                "2026-12-31T23:59:59-0100",
                "20261231T235959",
                "20270101T005959Z",
            ),
            (
                "2024-02-29T00:00:00+09:00",
                "20240229T000000",
                "20240228T150000Z",
            ),
            (
                "2026-03-01T00:00:00+01:00",
                "20260301T000000",
                "20260228T230000Z",
            ),
            ("2026-10-06T10:00:00", "20261006T100000", "20261006T100000Z"),
            (
                "2026-10-06 10:00:00.5+00:00",
                "20261006T100000",
                "20261006T100000Z",
            ),
        ];
        for (text, local, utc) in CASES {
            let stamp = Stamp::parse(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(stamp.local(), *local, "{text}");
            assert_eq!(stamp.utc(), *utc, "{text}");
        }
    }

    #[test]
    fn text_that_is_not_a_time_is_not_one() {
        for text in [
            "",
            "2026-10-06",
            "2026-13-06T10:00:00Z",
            "2026-10-32T10:00:00Z",
            "2026-10-06T24:00:00Z",
            "2026-10-06T10:60:00Z",
            "2026-10-06T10:00Z",
            "2026-10-06T10:00:00.Z",
            "2026-10-06T10:00:00+25:00",
            "2026-10-06T10:00:00X",
            "20261006T100000Z",
            "2026/10/06T10:00:00Z",
        ] {
            assert_eq!(Stamp::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_leap_second_is_the_last_second_of_its_minute() {
        let stamp = Stamp::parse("2016-12-31T23:59:60Z").expect("a leap second");
        assert_eq!(stamp.local(), "20161231T235959");
        assert_eq!(stamp.utc(), "20161231T235959Z");
    }

    #[test]
    fn a_day_the_calendar_lacks_is_not_a_time() {
        for text in [
            "2026-02-31T10:00:00Z",
            "2026-04-31T10:00:00Z",
            "2026-06-31T10:00:00+01:00",
            "2025-02-29T10:00:00Z",
            "2026-09-31T10:00:00",
        ] {
            assert_eq!(Stamp::parse(text), None, "{text:?}");
        }
        assert!(Stamp::parse("2024-02-29T10:00:00Z").is_some());
    }
}

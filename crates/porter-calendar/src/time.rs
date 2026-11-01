//! Dates and instants as the stores write them: a proleptic Gregorian date, and a moment in
//! seconds since the Unix epoch, always in UTC. chrono does the calendar arithmetic.

use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveTime, Timelike, Utc};
use std::fmt;

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
}

/// A moment, in seconds since 1970-01-01T00:00:00Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(pub i64);

impl Date {
    /// The date as chrono's. A `Date` built by hand with a day past its month's end counts on
    /// from the first of the month, as the arithmetic it replaces did; a month outside 1 to 12
    /// is the first day chrono can name.
    fn naive(self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, u32::from(self.month), u32::from(self.day))
            .or_else(|| {
                NaiveDate::from_ymd_opt(self.year, u32::from(self.month), 1)?
                    .checked_add_days(Days::new(u64::from(self.day.saturating_sub(1))))
            })
            .unwrap_or(NaiveDate::MIN)
    }

    fn of(date: NaiveDate) -> Self {
        Date {
            year: date.year(),
            month: date.month() as u8,
            day: date.day() as u8,
        }
    }
}

impl Date {
    /// The date written `YYYY-MM-DD`, or `None` for anything else or a day the month lacks.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().splitn(3, '-');
        let year = parts.next()?.parse().ok()?;
        let month: u8 = parts.next()?.parse().ok()?;
        let day: u8 = parts.next()?.parse().ok()?;
        NaiveDate::from_ymd_opt(year, u32::from(month), u32::from(day)).map(Date::of)
    }

    /// The day after.
    pub fn next(self) -> Self {
        Date::of(self.naive().succ_opt().unwrap_or(NaiveDate::MAX))
    }

    /// How many days from `self` to `other`: 1 for the next day.
    pub fn days_until(self, other: Date) -> i64 {
        (other.naive() - self.naive()).num_days()
    }

    /// Midnight UTC at the start of the day.
    pub fn midnight(self) -> Instant {
        Instant(self.naive().and_time(NaiveTime::MIN).and_utc().timestamp())
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl Instant {
    /// The date and time of day in UTC, written `YYYY-MM-DDTHH:MM:SS` with no zone: what Graph
    /// takes beside `"timeZone": "UTC"` and JMAP beside `Etc/UTC`.
    pub fn local_utc(self) -> String {
        let at = DateTime::from_timestamp(self.0, 0).unwrap_or(DateTime::<Utc>::MIN_UTC);
        format!(
            "{}T{:02}:{:02}:{:02}",
            Date::of(at.date_naive()),
            at.hour(),
            at.minute(),
            at.second()
        )
    }

    /// RFC 3339 in UTC: `YYYY-MM-DDTHH:MM:SSZ`.
    pub fn rfc3339(self) -> String {
        format!("{}Z", self.local_utc())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_written_and_counted() {
        let cases = [
            ("1970-01-01", Some((1970, 1, 1))),
            ("2024-02-29", Some((2024, 2, 29))),
            ("2023-02-29", None),
            ("2026-13-01", None),
            ("26-1-1", Some((26, 1, 1))),
            ("tomorrow", None),
        ];
        for (text, want) in cases {
            let got = Date::parse(text).map(|d| (d.year, d.month, d.day));
            assert_eq!(got, want, "{text}");
        }
        let leap = Date::parse("2024-02-28").unwrap();
        assert_eq!(leap.next().to_string(), "2024-02-29");
        assert_eq!(leap.next().next().to_string(), "2024-03-01");
        assert_eq!(
            Date::parse("2026-12-31").unwrap().next().to_string(),
            "2027-01-01"
        );
        assert_eq!(leap.days_until(Date::parse("2024-03-01").unwrap()), 2);
    }

    #[test]
    fn instants_are_written_in_utc() {
        assert_eq!(Instant(0).rfc3339(), "1970-01-01T00:00:00Z");
        assert_eq!(Instant(1_793_000_000).rfc3339(), "2026-10-26T07:33:20Z");
        assert_eq!(Instant(-1).local_utc(), "1969-12-31T23:59:59");
        assert_eq!(
            Date::parse("2026-10-26").unwrap().midnight().rfc3339(),
            "2026-10-26T00:00:00Z"
        );
    }
}

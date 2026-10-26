//! Dates and instants as the stores write them, without a calendar library: a proleptic
//! Gregorian date, and a moment in seconds since the Unix epoch, always in UTC.

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

/// Days since 1970-01-01 of a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> Date {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    Date { year, month, day }
}

impl Date {
    /// The date written `YYYY-MM-DD`, or `None` for anything else or a day the month lacks.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().splitn(3, '-');
        let year = parts.next()?.parse().ok()?;
        let month: u8 = parts.next()?.parse().ok()?;
        let day: u8 = parts.next()?.parse().ok()?;
        let date = Date { year, month, day };
        ((1..=12).contains(&month) && day >= 1 && civil_from_days(date.days()) == date)
            .then_some(date)
    }

    fn days(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// The day after.
    pub fn next(self) -> Self {
        civil_from_days(self.days() + 1)
    }

    /// How many days from `self` to `other`: 1 for the next day.
    pub fn days_until(self, other: Date) -> i64 {
        other.days() - self.days()
    }

    /// Midnight UTC at the start of the day.
    pub fn midnight(self) -> Instant {
        Instant(self.days() * 86_400)
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
        let date = civil_from_days(self.0.div_euclid(86_400));
        let second = self.0.rem_euclid(86_400);
        format!(
            "{date}T{:02}:{:02}:{:02}",
            second / 3600,
            second / 60 % 60,
            second % 60
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

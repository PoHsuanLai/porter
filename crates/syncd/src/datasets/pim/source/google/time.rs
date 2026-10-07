//! Dates and times as Google writes them (RFC 3339, or a bare `YYYY-MM-DD`) and as iCalendar
//! and vCard want them. Pure.

/// A parsed RFC 3339 time: the wall-clock fields as written and the offset they were written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    /// Minutes east of UTC.
    offset: i32,
}

fn digits(text: &str, from: usize, len: usize) -> Option<u32> {
    let part = text.get(from..from + len)?;
    part.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| part.parse().ok())?
}

fn valid_date(year: u32, month: u32, day: u32) -> bool {
    (1..=12).contains(&month) && (1..=31).contains(&day) && year >= 1
}

/// `20261006` from `2026-10-06` (anything after the date, such as a time, is ignored).
pub fn compact_date(text: &str) -> Option<String> {
    let (year, month, day) = (
        digits(text, 0, 4)?,
        digits(text, 5, 2)?,
        digits(text, 8, 2)?,
    );
    let dashes = text.as_bytes().get(4) == Some(&b'-') && text.as_bytes().get(7) == Some(&b'-');
    (dashes && valid_date(year, month, day)).then(|| format!("{year:04}{month:02}{day:02}"))
}

impl Stamp {
    /// Reads `2026-10-06T10:00:00-07:00`, with an optional fraction and `Z` for a zero offset.
    /// A time with no offset at all is read as UTC.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
        if !separators.iter().all(|(at, b)| bytes.get(*at) == Some(b))
            || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
        {
            return None;
        }
        let year = digits(text, 0, 4)?;
        let (month, day) = (digits(text, 5, 2)?, digits(text, 8, 2)?);
        let (hour, minute, second) = (
            digits(text, 11, 2)?,
            digits(text, 14, 2)?,
            digits(text, 17, 2)?,
        );
        if !valid_date(year, month, day) || hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let mut rest = text.get(19..)?;
        if let Some(fraction) = rest.strip_prefix('.') {
            let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if len == 0 {
                return None;
            }
            rest = &fraction[len..];
        }
        let offset = match rest.as_bytes() {
            [] | [b'Z' | b'z'] => 0,
            [sign @ (b'+' | b'-'), ..] => {
                let hours = digits(rest, 1, 2)?;
                let minutes = match rest.as_bytes().get(3) {
                    Some(b':') => digits(rest, 4, 2)?,
                    _ => digits(rest, 3, 2)?,
                };
                let total = i32::try_from(hours * 60 + minutes).ok()?;
                if hours > 23 || minutes > 59 {
                    return None;
                }
                if *sign == b'-' { -total } else { total }
            }
            _ => return None,
        };
        Some(Self {
            year: i64::from(year),
            month,
            day,
            hour,
            minute,
            second: second.min(59),
            offset,
        })
    }

    /// `20261006T100000`: the wall-clock time as written, for a `TZID=` property.
    pub fn local(&self) -> String {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `20261006T170000Z`: the same instant in UTC.
    pub fn utc(&self) -> String {
        let local = days_from_civil(self.year, self.month, self.day) * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        let at = local - i64::from(self.offset) * 60;
        let (days, secs) = (at.div_euclid(86_400), at.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
            secs / 3_600,
            secs % 3_600 / 60,
            secs % 60
        )
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
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
    fn a_date_is_compacted_and_a_bad_one_refused() {
        assert_eq!(compact_date("2026-10-06").as_deref(), Some("20261006"));
        assert_eq!(
            compact_date("2026-10-06T00:00:00.000Z").as_deref(),
            Some("20261006")
        );
        for text in [
            "",
            "2026-1-06",
            "2026/10/06",
            "2026-00-06",
            "2026-10-00",
            "abcd-ef-gh",
        ] {
            assert_eq!(compact_date(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_calendar_arithmetic_round_trips() {
        for days in [-1_000_000, -1, 0, 1, 59, 60, 365, 11_000, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}

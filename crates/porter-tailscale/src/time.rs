//! Go's `time.Time` as Tailscale writes it (RFC 3339, with a fraction and an offset), read to
//! whole Unix seconds. No date library: the one place a date is read is this file.

use porter_core::UnixSeconds;

/// Days from 1970-01-01 to the civil date (proleptic Gregorian), after Howard Hinnant's
/// `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn number(text: &str, digits: std::ops::RangeInclusive<usize>) -> Option<i64> {
    (digits.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The instant `text` names, in Unix seconds; `None` for text that is not an RFC 3339 time, and
/// for a time before 1970, which is how Go writes "never" (`0001-01-01T00:00:00Z`).
pub(crate) fn unix_seconds(text: &str) -> Option<UnixSeconds> {
    let (date, rest) = text.split_once(['T', 't'])?;
    let mut parts = date.split('-');
    let (year, month, day) = (
        number(parts.next()?, 4..=4)?,
        number(parts.next()?, 2..=2)?,
        number(parts.next()?, 2..=2)?,
    );
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // The clock, then the offset: `Z`, or `+hh:mm` / `-hh:mm`.
    let at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(at);
    let clock = clock
        .split_once('.')
        .map_or(clock, |(whole, _fraction)| whole);
    let mut fields = clock.split(':');
    let (hour, minute, second) = (
        number(fields.next()?, 2..=2)?,
        number(fields.next()?, 2..=2)?,
        number(fields.next()?, 2..=2)?,
    );
    if fields.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let offset = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, minutes) = zone[1..].split_once(':')?;
            sign * (number(hours, 2..=2)? * 3600 + number(minutes, 2..=2)? * 60)
        }
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    (seconds > 0).then_some(UnixSeconds(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_to_unix_seconds() {
        let cases = [
            ("1970-01-01T00:00:01Z", Some(1)),
            ("2000-02-29T00:00:00Z", Some(951_782_400)),
            ("2026-09-21T14:13:20Z", Some(1_790_000_000)),
            // A fraction is dropped, an offset is applied.
            ("2026-09-21T14:13:20.123456789Z", Some(1_790_000_000)),
            ("2026-09-21T22:13:20+08:00", Some(1_790_000_000)),
            ("2026-09-21T09:43:20-04:30", Some(1_790_000_000)),
            ("2026-09-21T14:13:20.5+00:00", Some(1_790_000_000)),
            // Go's zero time, and anything before 1970, is "never".
            ("0001-01-01T00:00:00Z", None),
            ("1969-12-31T23:59:59Z", None),
            // Not a time.
            ("", None),
            ("yesterday", None),
            ("2026-13-01T00:00:00Z", None),
            ("2026-09-21 10:13:20Z", None),
            ("2026-09-21T10:13Z", None),
            ("2026-09-21T10:13:20", None),
            ("2026-09-21T25:13:20Z", None),
        ];
        for (text, want) in cases {
            assert_eq!(unix_seconds(text), want.map(UnixSeconds), "{text:?}");
        }
    }
}

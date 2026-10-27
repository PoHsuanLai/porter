//! Units and counts: every number that means something has a type.

use serde::{Deserialize, Serialize};

macro_rules! unit {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

unit!(
    /// A number of model tokens (context window, output limit, usage).
    Tokens(u32)
);
unit!(
    /// The length of an embedding vector.
    Dims(u32)
);
unit!(
    /// Image pixels along one side.
    Px(u32)
);
unit!(
    /// A plain count of things (documents, users, languages).
    Count(u32)
);
unit!(
    /// A size in bytes.
    Bytes(u64)
);
unit!(
    /// Seconds since the Unix epoch, UTC. Instants on the wire and in stores; the clock that
    /// makes them is injected (`porter_core::clock::Clock`).
    UnixSeconds(i64)
);
unit!(
    /// Money in millionths of a US dollar: spend caps and price tables stay integers.
    MicroUsd(u64)
);
unit!(
    /// Thousandths of a whole: thresholds such as the spend warning at 800.
    Permille(u32)
);

impl UnixSeconds {
    /// The instant an RFC 3339 time names (`2026-10-06T10:00:00.123-07:00`, `…Z`), as the
    /// storage APIs write a file's last change. A fraction is dropped; `None` for anything else.
    pub fn from_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let number = |from, len| digits(text, from, len);
        let separated = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')]
            .iter()
            .all(|(at, b)| bytes.get(*at) == Some(b));
        if !separated || !matches!(bytes.get(10), Some(b'T' | b't')) {
            return None;
        }
        let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
        let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
        let date = (1..=12).contains(&month) && (1..=31).contains(&day);
        if !date || hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let mut rest = text.get(19..)?;
        if let Some(fraction) = rest.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            rest = &fraction[digits..];
        }
        let offset = match rest.as_bytes() {
            [b'Z' | b'z'] => 0,
            [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
                let (hours, minutes) = (digits(rest, 1, 2)?, digits(rest, 4, 2)?);
                if hours > 23 || minutes > 59 {
                    return None;
                }
                let east = hours * 3600 + minutes * 60;
                if *sign == b'-' { -east } else { east }
            }
            _ => return None,
        };
        let days = days_from_civil(year, month, day);
        Some(UnixSeconds(
            days * 86_400 + hour * 3600 + minute * 60 + second - offset,
        ))
    }
}

/// The number the `len` digits at `from` of `text` write.
fn digits(text: &str, from: usize, len: usize) -> Option<i64> {
    let part = text.get(from..from + len)?;
    match part.bytes().all(|b| b.is_ascii_digit()) {
        true => part.parse().ok(),
        false => None,
    }
}

/// Days from 1970-01-01 to the proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let of_year = (153 * shifted + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rfc3339_time_is_read_as_seconds_since_the_epoch() {
        const CASES: &[(&str, Option<i64>)] = &[
            ("1970-01-01T00:00:00Z", Some(0)),
            ("2026-10-24T05:42:18Z", Some(1_792_820_538)),
            ("2026-10-24T05:42:18.123Z", Some(1_792_820_538)),
            ("2026-10-24T13:42:18+08:00", Some(1_792_820_538)),
            ("2026-10-23T22:42:18-07:00", Some(1_792_820_538)),
            ("2000-02-29T00:00:00Z", Some(951_782_400)),
            ("1969-12-31T23:59:59Z", Some(-1)),
            ("2026-10-24", None),
            ("2026-10-24T05:42:18", None),
            ("2026-13-01T00:00:00Z", None),
            ("2026-10-24 05:42:18Z", None),
            ("2026-10-24T05:42:18.Z", None),
            ("not a time", None),
        ];
        for (text, want) in CASES {
            assert_eq!(
                UnixSeconds::from_rfc3339(text),
                want.map(UnixSeconds),
                "{text}"
            );
        }
    }
}

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
        let time = chrono::DateTime::parse_from_rfc3339(text).ok()?;
        Some(UnixSeconds(time.timestamp()))
    }

    /// `YYYY-MM-DD` of the UTC day this instant falls in; `None` for an instant outside the
    /// years chrono can write.
    pub fn utc_date(self) -> Option<String> {
        use chrono::Datelike;
        let day = chrono::DateTime::from_timestamp(self.0, 0)?.date_naive();
        Some(format!(
            "{:04}-{:02}-{:02}",
            day.year(),
            day.month(),
            day.day()
        ))
    }
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
            // RFC 3339 section 5.6 allows a space for the T; chrono reads it, as do the APIs' own clients.
            ("2026-10-24 05:42:18Z", Some(1_792_820_538)),
            ("2026-10-24t05:42:18z", Some(1_792_820_538)),
            ("2026-02-31T00:00:00Z", None),
            ("2026-04-31T00:00:00Z", None),
            ("2026-10-24T24:00:00Z", None),
            ("2026-10-24T05:42:18+24:00", None),
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

    #[test]
    fn the_utc_day_of_a_unix_time() {
        let cases = [
            (0, "1970-01-01"),
            (86_399, "1970-01-01"),
            (86_400, "1970-01-02"),
            (951_782_400, "2000-02-29"),
            (1_790_000_000, "2026-09-21"),
            (-86_400, "1969-12-31"),
        ];
        for (at, want) in cases {
            assert_eq!(UnixSeconds(at).utc_date().as_deref(), Some(want), "{at}");
        }
        assert_eq!(UnixSeconds(i64::MAX).utc_date(), None);
    }
}

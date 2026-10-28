//! Go's `time.Time` as Tailscale writes it (RFC 3339, with a fraction and an offset), read to
//! whole Unix seconds by `UnixSeconds::from_rfc3339` (chrono).

use porter_ids::UnixSeconds;

/// The instant `text` names, in Unix seconds; `None` for text that is not an RFC 3339 time, and
/// for a time before 1970, which is how Go writes "never" (`0001-01-01T00:00:00Z`).
pub(crate) fn unix_seconds(text: &str) -> Option<UnixSeconds> {
    UnixSeconds::from_rfc3339(text).filter(|at| at.0 > 0)
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
            // RFC 3339 allows a space for the T.
            ("2026-09-21 10:13:20Z", Some(1_789_985_600)),
            ("2026-09-21T10:13Z", None),
            ("2026-09-21T10:13:20", None),
            ("2026-09-21T25:13:20Z", None),
        ];
        for (text, want) in cases {
            assert_eq!(unix_seconds(text), want.map(UnixSeconds), "{text:?}");
        }
    }
}

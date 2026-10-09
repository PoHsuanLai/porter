//! A seeded random source and the delays built on it: exponential backoff with jitter. No
//! system randomness is read, so a test names the seed and gets the same delays.

/// SplitMix64: a small, well-mixed generator. Not for secrets; for spreading wake-ups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jitter(u64);

impl Jitter {
    /// A source starting from `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `low..=high` (`low` when the range is empty or reversed).
    pub fn between(&mut self, low: u32, high: u32) -> u32 {
        if high <= low {
            return low;
        }
        let span = u64::from(high - low) + 1;
        low + u32::try_from(self.next() % span).unwrap_or(0)
    }
}

/// `base * 2^steps`, never more than `cap` (all seconds, saturating).
pub fn exponential(base: u32, steps: u32, cap: u32) -> u32 {
    let factor = 1u64.checked_shl(steps.min(32)).unwrap_or(u64::MAX);
    let delay = u64::from(base).saturating_mul(factor);
    u32::try_from(delay.min(u64::from(cap))).unwrap_or(cap)
}

/// How long a supervisor waits before it looks again: the `rescan` interval when its last look
/// went well (`failures` is 0), else a backoff from 5 s (doubling per failed look in a row) that
/// never passes `rescan`. A grant that could not be mirrored for a passing reason (a slow
/// machine, a full disk for a moment, accountd not yet up) is tried again in seconds, not at
/// the next ten-minute rescan.
pub fn next_look(rescan: std::time::Duration, failures: u32) -> std::time::Duration {
    if failures == 0 {
        return rescan;
    }
    let cap = u32::try_from(rescan.as_secs()).unwrap_or(u32::MAX).max(1);
    let seconds = exponential(5, failures - 1, cap);
    std::time::Duration::from_secs(u64::from(seconds)).min(rescan)
}

/// `delay` seconds with "equal jitter": half of it fixed, the other half random, so retries of
/// many datasets spread out but none comes before half its delay.
pub fn jittered(delay: u32, source: &mut Jitter) -> u32 {
    let half = delay / 2;
    half + source.between(0, delay - half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_look_is_repeated_in_seconds_and_backs_off_to_the_rescan() {
        use std::time::Duration;
        let ten_minutes = Duration::from_secs(600);
        assert_eq!(next_look(ten_minutes, 0), ten_minutes);
        let waits: Vec<u64> = (1..=9)
            .map(|n| next_look(ten_minutes, n).as_secs())
            .collect();
        assert_eq!(waits, [5, 10, 20, 40, 80, 160, 320, 600, 600]);
        assert_eq!(next_look(ten_minutes, u32::MAX), ten_minutes);
        // A short rescan (a test build's) is never exceeded.
        assert_eq!(next_look(Duration::from_secs(2), 3), Duration::from_secs(2));
    }

    #[test]
    fn the_same_seed_gives_the_same_stream_and_a_range_is_respected() {
        let (mut a, mut b) = (Jitter::seeded(7), Jitter::seeded(7));
        let drawn: Vec<u32> = (0..20).map(|_| a.between(10, 20)).collect();
        assert_eq!(
            drawn,
            (0..20).map(|_| b.between(10, 20)).collect::<Vec<_>>()
        );
        assert!(drawn.iter().all(|n| (10..=20).contains(n)));
        assert!(drawn.iter().any(|n| *n != drawn[0]), "it does vary");
        assert_eq!(Jitter::seeded(1).between(5, 5), 5);
        assert_eq!(Jitter::seeded(1).between(9, 3), 9);
    }

    #[test]
    fn backoff_doubles_to_its_cap() {
        const CASES: &[(u32, u32, u32, u32)] = &[
            (30, 0, 3600, 30),
            (30, 1, 3600, 60),
            (30, 3, 3600, 240),
            (30, 7, 3600, 3600),
            (30, 40, 3600, 3600),
            (u32::MAX, 5, 1000, 1000),
            (0, 5, 1000, 0),
        ];
        for (base, steps, cap, want) in CASES {
            assert_eq!(
                exponential(*base, *steps, *cap),
                *want,
                "{base} {steps} {cap}"
            );
        }
    }

    #[test]
    fn jitter_keeps_at_least_half_the_delay_and_at_most_all_of_it() {
        let mut source = Jitter::seeded(99);
        for delay in [0, 1, 2, 60, 3600] {
            for _ in 0..50 {
                let got = jittered(delay, &mut source);
                assert!(got >= delay / 2 && got <= delay, "{delay} -> {got}");
            }
        }
    }
}

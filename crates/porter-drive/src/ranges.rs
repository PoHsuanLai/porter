//! The byte ranges of a file that are in the cache: sorted, merged, half-open `[start, end)`.

use serde::{Deserialize, Serialize};

/// Byte ranges, merged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Ranges(Vec<[u64; 2]>);

impl Ranges {
    /// All of `[0, size)`.
    pub(crate) fn whole(size: u64) -> Self {
        match size {
            0 => Self::default(),
            size => Self(vec![[0, size]]),
        }
    }

    /// Adds `[start, end)`.
    pub(crate) fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut start, mut end) = (start, end);
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for &[a, b] in &self.0 {
            if b < start || a > end {
                kept.push([a, b]);
            } else {
                start = start.min(a);
                end = end.max(b);
            }
        }
        kept.push([start, end]);
        kept.sort_unstable();
        self.0 = kept;
    }

    /// Whether all of `[start, end)` is there.
    pub(crate) fn covers(&self, start: u64, end: u64) -> bool {
        start >= end || self.0.iter().any(|&[a, b]| a <= start && end <= b)
    }

    /// How many bytes are there.
    pub(crate) fn bytes(&self) -> u64 {
        self.0.iter().map(|&[a, b]| b - a).sum()
    }

    /// The first byte from `from` on, below `size`, that is not there.
    pub(crate) fn first_gap(&self, from: u64, size: u64) -> Option<u64> {
        let mut at = from;
        for &[a, b] in &self.0 {
            if a <= at && at < b {
                at = b;
            }
        }
        (at < size).then_some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_say_what_is_missing() {
        let mut ranges = Ranges::default();
        ranges.insert(10, 20);
        ranges.insert(0, 5);
        ranges.insert(5, 10);
        assert_eq!(ranges, Ranges(vec![[0, 20]]));
        ranges.insert(30, 40);
        assert!(ranges.covers(2, 18) && !ranges.covers(15, 35));
        assert_eq!(ranges.first_gap(0, 50), Some(20));
        assert_eq!(ranges.first_gap(30, 40), None);
        assert_eq!(ranges.first_gap(35, 50), Some(40));
        assert_eq!(ranges.bytes(), 30);
        assert!(Ranges::whole(9).covers(0, 9));
    }
}

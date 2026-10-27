//! The byte ranges of a file that are in the cache: sorted, merged, half-open `[start, end)`.

use rangemap::RangeSet;
use serde::{Deserialize, Serialize};

/// Byte ranges, merged. On disk they are `[[start, end], ...]`, sorted, as `index.json` has always
/// held them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<[u64; 2]>", into = "Vec<[u64; 2]>")]
pub(crate) struct Ranges(RangeSet<u64>);

impl From<Vec<[u64; 2]>> for Ranges {
    fn from(pairs: Vec<[u64; 2]>) -> Self {
        let mut ranges = Self::default();
        for [start, end] in pairs {
            ranges.insert(start, end);
        }
        ranges
    }
}

impl From<Ranges> for Vec<[u64; 2]> {
    fn from(ranges: Ranges) -> Self {
        ranges.0.iter().map(|r| [r.start, r.end]).collect()
    }
}

impl Ranges {
    /// All of `[0, size)`.
    pub(crate) fn whole(size: u64) -> Self {
        let mut ranges = Self::default();
        ranges.insert(0, size);
        ranges
    }

    /// Adds `[start, end)`.
    pub(crate) fn insert(&mut self, start: u64, end: u64) {
        if start < end {
            self.0.insert(start..end);
        }
    }

    /// Whether all of `[start, end)` is there.
    pub(crate) fn covers(&self, start: u64, end: u64) -> bool {
        start >= end || self.0.gaps(&(start..end)).next().is_none()
    }

    /// How many bytes are there.
    pub(crate) fn bytes(&self) -> u64 {
        self.0.iter().map(|r| r.end - r.start).sum()
    }

    /// The first byte from `from` on, below `size`, that is not there.
    pub(crate) fn first_gap(&self, from: u64, size: u64) -> Option<u64> {
        (from < size)
            .then(|| self.0.gaps(&(from..size)).next().map(|gap| gap.start))
            .flatten()
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
        assert_eq!(Vec::from(ranges.clone()), vec![[0, 20]]);
        ranges.insert(30, 40);
        assert!(ranges.covers(2, 18) && !ranges.covers(15, 35));
        assert_eq!(ranges.first_gap(0, 50), Some(20));
        assert_eq!(ranges.first_gap(30, 40), None);
        assert_eq!(ranges.first_gap(35, 50), Some(40));
        assert_eq!(ranges.bytes(), 30);
        assert!(Ranges::whole(9).covers(0, 9));
    }

    #[test]
    fn an_index_file_written_before_reads_and_writes_back_the_same_shape() {
        let text = r#"[[0,20],[30,40]]"#;
        let ranges: Ranges = serde_json::from_str(text).expect("an index's ranges");
        assert_eq!(ranges.bytes(), 30);
        assert!(ranges.covers(0, 20) && !ranges.covers(19, 31));
        assert_eq!(serde_json::to_string(&ranges).expect("json"), text);
        // A file that was never merged is merged when read.
        let loose: Ranges = serde_json::from_str("[[30,40],[0,10],[10,20],[5,5]]").expect("loose");
        assert_eq!(serde_json::to_string(&loose).expect("json"), text);
        assert_eq!(
            serde_json::to_string(&Ranges::default()).expect("json"),
            "[]"
        );
    }

    #[test]
    fn a_stored_row_keeps_its_shape() {
        let text =
            r#"{"version":"v1","size":40,"present":[[0,20],[30,40]],"opened":3,"checked":false}"#;
        let row: crate::index::Row = serde_json::from_str(text).expect("a row");
        assert_eq!(serde_json::to_string(&row).expect("json"), text);
    }
}

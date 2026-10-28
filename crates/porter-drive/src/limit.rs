//! How much the cache may hold of what is not kept downloaded.

use std::path::Path;

/// The least the automatic limit is.
pub const AUTOMATIC_FLOOR: u64 = 1 << 30;

/// The cache's limit: past it, what is not kept downloaded and not open is removed, oldest
/// opened first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheLimit {
    /// A tenth of the file system the cache is on, and at least [`AUTOMATIC_FLOOR`].
    #[default]
    Automatic,
    /// This many bytes.
    Bytes(u64),
}

impl CacheLimit {
    /// The limit a setting in bytes names: 0 is automatic.
    pub fn from_setting(bytes: u64) -> Self {
        match bytes {
            0 => CacheLimit::Automatic,
            bytes => CacheLimit::Bytes(bytes),
        }
    }

    /// The setting in bytes this limit is written as: 0 for automatic.
    pub fn setting(self) -> u64 {
        match self {
            CacheLimit::Automatic => 0,
            CacheLimit::Bytes(bytes) => bytes,
        }
    }

    /// The limit in bytes for a cache in `dir` (a directory that may not exist yet: its nearest
    /// existing parent is measured).
    pub fn bytes_for(self, dir: &Path) -> u64 {
        match self {
            CacheLimit::Bytes(bytes) => bytes,
            CacheLimit::Automatic => {
                let total = dir
                    .ancestors()
                    .find_map(|at| rustix::fs::statvfs(at).ok())
                    .map_or(0, |fs| fs.f_blocks.saturating_mul(fs.f_frsize));
                (total / 10).max(AUTOMATIC_FLOOR)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_automatic_and_automatic_is_at_least_a_gibibyte() {
        assert_eq!(CacheLimit::from_setting(0), CacheLimit::Automatic);
        assert_eq!(CacheLimit::from_setting(5).setting(), 5);
        let dir = test_kit::Scratch::new("limit");
        assert!(CacheLimit::Automatic.bytes_for(&dir.join("not/yet")) >= AUTOMATIC_FLOOR);
        assert_eq!(CacheLimit::Bytes(7).bytes_for(dir.path()), 7);
    }
}

//! Where a host keeps its registry: the directory it chooses.

use std::path::{Path, PathBuf};

/// The directory a [`crate::FileStore`] keeps its files in (created on the first save).
///
/// The host chooses it and the library reads no environment and no XDG variable: accountd's
/// `Config::from_env` resolves `$XDG_STATE_HOME/porter`, and an app names its own state
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreLocation(PathBuf);

impl StoreLocation {
    /// The directory `dir` names.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(dir.into())
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.0
    }
}

impl From<PathBuf> for StoreLocation {
    fn from(dir: PathBuf) -> Self {
        Self(dir)
    }
}

impl From<&Path> for StoreLocation {
    fn from(dir: &Path) -> Self {
        Self(dir.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_is_the_directory_it_was_given() {
        let dir = PathBuf::from("/state/porter");
        assert_eq!(StoreLocation::new(&dir).dir(), dir);
        assert_eq!(StoreLocation::from(dir.clone()), StoreLocation::new(&dir));
        assert_eq!(StoreLocation::from(dir.as_path()), StoreLocation::new(dir));
    }
}

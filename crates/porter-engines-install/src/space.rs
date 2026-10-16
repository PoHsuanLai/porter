//! Whether there is room, before a long download.

use std::path::Path;

/// Not enough free space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("needs {need} bytes and {free} are free")]
pub struct NoSpace {
    /// Bytes the install needs.
    pub need: u64,
    /// Bytes free.
    pub free: u64,
}

/// The bytes free to a program that is not root on the file system holding `path`, or on the
/// nearest of its parents that exists. `None` when it cannot be read.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let near = path.ancestors().find(|ancestor| ancestor.exists())?;
    let stat = rustix::fs::statvfs(near).ok()?;
    stat.f_bavail.checked_mul(stat.f_frsize)
}

/// Whether `need` bytes are free at `path`. A file system that cannot be read is let through:
/// the writes will say.
///
/// # Errors
/// [`NoSpace`] when fewer bytes are free.
pub fn require_space(path: &Path, need: u64) -> Result<(), NoSpace> {
    match free_bytes(path) {
        Some(free) if free < need => Err(NoSpace { need, free }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_fake::Scratch;

    #[test]
    fn room_is_asked_of_the_nearest_existing_parent() {
        let dir = Scratch::new("space");
        let deep = dir.path().join("a/b/c");
        assert!(free_bytes(&deep).is_some());
        assert_eq!(require_space(&deep, 1), Ok(()));
        let huge = require_space(&deep, u64::MAX);
        assert!(matches!(huge, Err(NoSpace { need: u64::MAX, .. })));
    }
}

//! Unpacking a pinned `tar.gz` with the system's own `tar`, and settling the modes of what it
//! made. Only an archive whose SHA-256 is the pinned one is ever unpacked here.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the system's `tar` is: fixed paths, never a search of the environment.
const TAR_PATHS: [&str; 2] = ["/usr/bin/tar", "/bin/tar"];

/// Why an archive was not unpacked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    /// This computer has no `tar` at its usual place.
    #[error("this computer has no tar program")]
    NoTar,
    /// `tar` ended in failure.
    #[error("the archive could not be unpacked")]
    Failed,
    /// The disk refused.
    #[error("the disk refused the unpacking: {0}")]
    Disk(std::io::ErrorKind),
}

fn tar_program() -> Result<PathBuf, ExtractError> {
    TAR_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or(ExtractError::NoTar)
}

/// Unpacks `archive` into the directory `into`, which must exist, without its top directory.
///
/// # Errors
/// See [`ExtractError`].
pub fn unpack(archive: &Path, into: &Path) -> Result<(), ExtractError> {
    let status = Command::new(tar_program()?)
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .args([
            "--strip-components=1",
            "--no-same-owner",
            "--no-same-permissions",
        ])
        .env_clear()
        // `tar -z` runs `gzip`, which it looks for on this path and no other.
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| ExtractError::Disk(e.kind()))?;
    if status.success() {
        Ok(())
    } else {
        Err(ExtractError::Failed)
    }
}

/// Sets every directory under `root` (and `root`) to 0755, every file with an execute bit to
/// 0755 and every other file to 0644, whatever the umask made of them. Links are left alone.
///
/// # Errors
/// [`ExtractError::Disk`] when a mode cannot be set.
pub fn settle_modes(root: &Path) -> Result<(), ExtractError> {
    let disk = |e: std::io::Error| ExtractError::Disk(e.kind());
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).map_err(disk)?;
        for entry in std::fs::read_dir(&dir).map_err(disk)? {
            let path = entry.map_err(disk)?.path();
            let meta = std::fs::symlink_metadata(&path).map_err(disk)?;
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                let mode = if meta.permissions().mode() & 0o111 == 0 {
                    0o644
                } else {
                    0o755
                };
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                    .map_err(disk)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_fake::Scratch;

    #[test]
    fn an_archive_is_unpacked_without_its_top_directory_and_modes_are_settled() {
        let dir = Scratch::new("extract");
        let top = dir.path().join("top");
        std::fs::create_dir_all(top.join("sub")).expect("dir");
        std::fs::write(top.join("run"), "#!/bin/sh\n").expect("run");
        std::fs::write(top.join("sub/data"), "d").expect("data");
        std::os::unix::fs::symlink("run", top.join("link")).expect("link");
        std::fs::set_permissions(top.join("run"), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        let archive = dir.path().join("a.tar.gz");
        let made = Command::new(tar_program().expect("tar"))
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .arg("top")
            .status()
            .expect("tar runs");
        assert!(made.success());
        let into = dir.path().join("into");
        std::fs::create_dir(&into).expect("dir");
        unpack(&archive, &into).expect("unpack");
        settle_modes(&into).expect("modes");
        let mode = |name: &str| {
            std::fs::metadata(into.join(name))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(
            (mode("run"), mode("sub/data"), mode("sub")),
            (0o755, 0o644, 0o755)
        );
        assert_eq!(
            std::fs::read_link(into.join("link")).expect("link"),
            Path::new("run")
        );
        assert!(!into.join("top").exists());
        assert_eq!(
            unpack(&dir.path().join("gone.tar.gz"), &into),
            Err(ExtractError::Failed)
        );
    }
}

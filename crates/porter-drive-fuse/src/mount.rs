//! Mounting through the system's `fusermount3`, which fuser runs as a separate process.

use crate::fs::{DriveFs, Mounted as Served};
use fuser::{BackgroundSession, Config, MountOption};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

/// Why a drive was not mounted.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MountError {
    /// This computer has no FUSE device.
    #[error("there is no {0}")]
    NoDevice(PathBuf),
    /// This computer has no `fusermount3` to run.
    #[error("{0} cannot be run")]
    NoHelper(String),
    /// The folder could not be made.
    #[error("the folder could not be made: {0}")]
    Folder(#[source] std::io::Error),
    /// Mounting failed.
    #[error("mounting failed: {0}")]
    Failed(#[source] std::io::Error),
}

/// How drives are mounted on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mounter {
    /// The FUSE device.
    pub device: PathBuf,
    /// The helper that mounts and unmounts, found on `PATH`.
    pub helper: String,
}

impl Default for Mounter {
    fn default() -> Self {
        Self {
            device: PathBuf::from("/dev/fuse"),
            helper: "fusermount3".to_owned(),
        }
    }
}

/// Whether `path` is a mount point now, as `/proc/self/mountinfo` says.
pub fn is_mount_point(path: &Path) -> bool {
    let Ok(table) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    table.lines().any(|line| {
        line.split(' ')
            .nth(4)
            .is_some_and(|point| Path::new(&unescape(point)) == path)
    })
}

/// A mountinfo field with its octal escapes (`\040` for a space) read back.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Ok(code) = u8::from_str_radix(&field[i + 1..i + 4], 8)
        {
            out.push(code);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Mounter {
    /// Whether this computer can mount: the device is there and the helper runs.
    ///
    /// # Errors
    /// What is missing.
    pub fn check(&self) -> Result<(), MountError> {
        if !self.device.exists() {
            return Err(MountError::NoDevice(self.device.clone()));
        }
        let runs = Command::new(&self.helper)
            .arg("-V")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        match runs {
            true => Ok(()),
            false => Err(MountError::NoHelper(self.helper.clone())),
        }
    }

    /// Unmounts a mount left at `path` (a crash leaves one that answers nothing), lazily.
    pub fn clean(&self, path: &Path) {
        let stale = is_mount_point(path)
            || std::fs::metadata(path).is_err_and(|e| {
                e.raw_os_error() == Some(rustix::io::Errno::NOTCONN.raw_os_error())
            });
        if stale {
            let _ = Command::new(&self.helper)
                .args(["-u", "-z"])
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    /// Mounts `fs` at `path`, read-only, after cleaning a stale mount there and making the folder.
    ///
    /// # Errors
    /// No device or helper, the folder could not be made, or the mount failed.
    pub fn mount(&self, fs: DriveFs, path: &Path) -> Result<Mounted, MountError> {
        self.check()?;
        self.clean(path);
        std::fs::create_dir_all(path).map_err(MountError::Folder)?;
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::RO,
            MountOption::NoSuid,
            MountOption::NoDev,
            MountOption::NoAtime,
            MountOption::FSName("porter".to_owned()),
            MountOption::Subtype("porter".to_owned()),
        ];
        let session =
            fuser::spawn_mount(Served(Arc::new(fs)), path, &config).map_err(MountError::Failed)?;
        Ok(Mounted {
            session: Some(session),
            path: path.to_path_buf(),
        })
    }
}

/// A drive mounted at its path: unmounted when dropped.
#[derive(Debug)]
pub struct Mounted {
    session: Option<BackgroundSession>,
    path: PathBuf,
}

impl Mounted {
    /// Where it is mounted.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Unmounts it now.
    ///
    /// # Errors
    /// The unmount failed (the mount stays until the process ends).
    pub fn unmount(mut self) -> std::io::Result<()> {
        match self.session.take() {
            Some(session) => session.umount_and_join(),
            None => Ok(()),
        }
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if let Some(session) = self.session.take()
            && let Err(why) = session.umount_and_join()
        {
            eprintln!(
                "porter-drive-fuse: {} was not unmounted: {why}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_computer_with_no_device_or_helper_is_told_apart_and_nothing_is_mounted() {
        let dir = porter_fake::Scratch::new("drive-fuse-mount");
        let none = Mounter {
            device: dir.join("no-fuse"),
            helper: "fusermount3".into(),
        };
        assert!(matches!(none.check(), Err(MountError::NoDevice(_))));
        let no_helper = Mounter {
            device: dir.path().to_path_buf(),
            helper: dir.join("no-helper").display().to_string(),
        };
        assert!(matches!(no_helper.check(), Err(MountError::NoHelper(_))));
        assert!(!is_mount_point(&dir.join("drive")));
        assert_eq!(
            unescape("/home/ada/Cloud/Google\\040Drive"),
            "/home/ada/Cloud/Google Drive"
        );
    }
}

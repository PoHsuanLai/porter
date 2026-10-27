//! Mounting: through porter-mountd when it listens (it runs `fusermount3` where syncd's sandbox
//! may not, and hands back the connection), or else through the system's `fusermount3`, which
//! fuser runs as a separate process.

use crate::fs::{DriveFs, Mounted as Served};
use fuser::{BackgroundSession, Config, MountOption, Session, SessionACL};
use porter_mountd::client::{self, AskError};
use porter_mountd::mountinfo;
use porter_mountd::wire::{Access, Code, FSTYPE, FolderName};
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
    /// porter-mountd refused, for this reason.
    #[error("porter-mountd refused: {0}")]
    Refused(Code),
}

/// How drives are mounted on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mounter {
    /// The FUSE device.
    pub device: PathBuf,
    /// The helper that mounts and unmounts: its path, or a name the host chose to leave to the
    /// system to find.
    pub helper: String,
    /// porter-mountd's socket, when the drive is to be mounted through it
    /// (`$XDG_RUNTIME_DIR/porter/mountd.sock`, [`porter_mountd::wire::socket_in`]). It puts the
    /// folder at `~/Cloud/<name>`, so the path mounted must be that. When nothing listens there,
    /// the drive is mounted directly, as with none.
    pub mountd: Option<PathBuf>,
}

impl Mounter {
    /// Mounting with `helper` on the usual FUSE device, directly (no porter-mountd).
    pub fn new(helper: impl Into<String>) -> Self {
        Self {
            device: PathBuf::from("/dev/fuse"),
            helper: helper.into(),
            mountd: None,
        }
    }
}

/// Whether `path` is a mount point now, as `/proc/self/mountinfo` says.
pub fn is_mount_point(path: &Path) -> bool {
    porter_mountd::mountinfo::at(&porter_mountd::mountinfo::read(), path).is_some()
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

    /// Unmounts a drive's folder left at `path` (a crash leaves one that answers nothing), lazily.
    /// Only a drive's folder (`fuse.porter`) is touched: a mount of anything else there, the
    /// person's own, is left as it is.
    pub fn clean(&self, path: &Path) {
        if ours_at(path).is_some() {
            let _ = take_away_directly(&self.helper, path);
        }
    }

    /// Mounts `fs` at `path`, read-write over a drive that takes changes and read-only over one
    /// that does not: through porter-mountd when it listens at [`mountd`](Self::mountd), which
    /// cleans a stale mount there and makes the folder; else directly, which does the same here.
    ///
    /// # Errors
    /// porter-mountd refused; or, mounting directly, no device or helper, the folder could not
    /// be made, or the mount failed.
    pub fn mount(&self, fs: DriveFs, path: &Path) -> Result<Mounted, MountError> {
        let access = match fs.writable() {
            true => Access::ReadWrite,
            false => Access::ReadOnly,
        };
        if let Some(socket) = &self.mountd {
            match helped(socket, path) {
                Ok(through) => {
                    let fd = client::mount(socket, &through.name, access);
                    match fd {
                        Ok(fd) => return serve_over(fs, fd, path, through),
                        Err(AskError::Absent) => {}
                        Err(AskError::Refused(code)) => return Err(MountError::Refused(code)),
                        Err(AskError::Broken(why)) => return Err(MountError::Failed(why)),
                        Err(why) => return Err(MountError::Failed(std::io::Error::other(why))),
                    }
                }
                Err(why) => return Err(MountError::Failed(why)),
            }
        }
        self.check()?;
        self.clean(path);
        if let Some(entry) = mountinfo::at(&mountinfo::read(), path) {
            return Err(MountError::Refused(match entry.fstype == FSTYPE {
                true => Code::AlreadyShown,
                false => Code::FolderInUse,
            }));
        }
        std::fs::create_dir_all(path).map_err(MountError::Folder)?;
        let mut config = Config::default();
        config.mount_options = vec![
            match access {
                Access::ReadWrite => MountOption::RW,
                _ => MountOption::RO,
            },
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
            by: By::Directly(self.helper.clone()),
        })
    }
}

/// The drive's folder (`fuse.porter`) at `path`, if one is mounted there.
fn ours_at(path: &Path) -> Option<mountinfo::Entry> {
    mountinfo::at(&mountinfo::read(), path)
        .filter(|entry| entry.fstype == FSTYPE)
        .cloned()
}

/// Unmounts the drive's folder at `path` with `helper` (`fusermount3`), lazily, and ends its
/// connection where this process may (`/sys/fs/fuse/connections`), so the session serving it
/// ends at once even while a file there is open.
fn take_away_directly(helper: &str, path: &Path) -> std::io::Result<()> {
    let connection = ours_at(path).map(|entry| entry.connection());
    let output = Command::new(helper)
        .args(["-u", "-z", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "{helper}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if let Some(connection) = connection {
        let _ = mountinfo::abort(Path::new(mountinfo::FUSE_CONNECTIONS), connection);
    }
    Ok(())
}

/// The folder name porter-mountd is asked for: `path`'s last part.
fn helped(socket: &Path, path: &Path) -> std::io::Result<Through> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| FolderName::parse(name).ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} has no folder name porter-mountd takes", path.display()),
            )
        })?;
    Ok(Through {
        socket: socket.to_path_buf(),
        name,
    })
}

/// Serves `fs` over the connection porter-mountd mounted.
fn serve_over(
    fs: DriveFs,
    fd: std::os::fd::OwnedFd,
    path: &Path,
    through: Through,
) -> Result<Mounted, MountError> {
    let started = Session::from_fd(
        Served(Arc::new(fs)),
        fd,
        SessionACL::Owner,
        Config::default(),
    )
    .and_then(Session::spawn);
    match started {
        Ok(session) => Ok(Mounted {
            session: Some(session),
            path: path.to_path_buf(),
            by: By::Helper(through),
        }),
        Err(why) => {
            // The folder is mounted but nothing will serve it: take it away again.
            let _ = client::unmount(&through.socket, &through.name);
            Err(MountError::Failed(why))
        }
    }
}

/// A mount porter-mountd made: taken away by asking it again.
#[derive(Debug)]
struct Through {
    socket: PathBuf,
    name: FolderName,
}

/// Who mounted a drive's folder, and so takes it away.
#[derive(Debug)]
enum By {
    /// porter-mountd.
    Helper(Through),
    /// This process, with this `fusermount3`.
    Directly(String),
}

/// A drive mounted at its path: unmounted when dropped.
///
/// Taking it away never waits for the session serving it: the folder is unmounted and its
/// connection ended, then the session's thread is left to finish on its own (it ends when the
/// connection does, and drops the drive). So a caller on an async runtime is never held, even
/// while a file in the folder is still open.
#[derive(Debug)]
pub struct Mounted {
    session: Option<BackgroundSession>,
    path: PathBuf,
    by: By,
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
        self.take_away()
    }

    /// Unmounts it now, keeping it as it was when that fails, so it can be asked again.
    ///
    /// # Errors
    /// The unmount failed: it is still mounted, and still served.
    pub fn try_unmount(&mut self) -> std::io::Result<()> {
        if self.session.is_none() {
            return Ok(());
        }
        self.unmount_now()?;
        self.let_go();
        Ok(())
    }

    fn take_away(&mut self) -> std::io::Result<()> {
        if self.session.is_none() {
            return Ok(());
        }
        let taken = self.unmount_now();
        self.let_go();
        taken
    }

    fn unmount_now(&self) -> std::io::Result<()> {
        match &self.by {
            By::Helper(through) => {
                client::unmount(&through.socket, &through.name).map_err(std::io::Error::other)
            }
            By::Directly(helper) => take_away_directly(helper, &self.path),
        }
    }

    /// Its session's thread ends when the connection does: left to finish on a thread of its
    /// own, never joined here. If the folder was not taken away, it goes on serving it until
    /// the process ends and the connection closes with it.
    fn let_go(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = std::thread::Builder::new()
                .name("porter-drive-fuse-end".to_owned())
                .spawn(move || drop(session.join()));
        }
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if let Err(why) = self.take_away() {
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
        let dir = test_kit::Scratch::new("drive-fuse-mount");
        let none = Mounter {
            device: dir.join("no-fuse"),
            ..Mounter::new("fusermount3")
        };
        assert!(matches!(none.check(), Err(MountError::NoDevice(_))));
        let no_helper = Mounter {
            device: dir.path().to_path_buf(),
            helper: dir.join("no-helper").display().to_string(),
            mountd: None,
        };
        assert!(matches!(no_helper.check(), Err(MountError::NoHelper(_))));
        assert!(!is_mount_point(&dir.join("drive")));
    }
}

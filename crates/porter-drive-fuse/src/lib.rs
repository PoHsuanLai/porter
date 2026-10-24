//! A porter-drive [`Drive`](porter_drive::Drive) at a real path, through FUSE.
//!
//! - [`DriveFs`]: fuser's calls mapped onto the core. Every call has a plain method of its own
//!   ([`DriveFs::lookup`], [`DriveFs::getattr`], [`DriveFs::readdir`], [`DriveFs::open`],
//!   [`DriveFs::read`], [`DriveFs::release`], [`DriveFs::statfs`]) that the tests drive with no
//!   mount; the `fuser::Filesystem` impl only calls them and replies. The mount is read-only:
//!   every write answers `EROFS`. Inode numbers are the core's, stable per remote id; mtime is
//!   the drive's `modified`; a size is the drive's, or an online document's link file's. A read
//!   that waits for its chunk runs on the Tokio runtime and replies from there, so the file
//!   system keeps answering meanwhile. A read offline fails with `ENETDOWN`, any other failed
//!   fetch with `EIO`.
//! - [`Mounter`]: mounts through porter-mountd when it listens (syncd's unit may not run the
//!   setuid `fusermount3`; the helper does and hands back the open connection, which fuser's
//!   `Session::from_fd` serves), and else through the system's `fusermount3`, run as a separate
//!   process (fuser with no default features links no libfuse). A stale drive's folder at the
//!   path (`fuse.porter`), left by a crash, is unmounted first (`fusermount3 -u -z`); a mount of
//!   anything else there is never touched. No `/dev/fuse`, no `fusermount3`, a refusal, or a
//!   mount that fails is a [`MountError`] with the words Files shows. [`Mounted`] unmounts when
//!   dropped and ends the connection, without waiting for the session.

mod fs;
mod mount;

/// porter-mountd's client and wire format, for a caller that names its socket.
pub use porter_mountd as mountd;

pub use fs::{Attr, DirEntry, DriveFs, Space, errno_of};
pub use mount::{MountError, Mounted, Mounter, is_mount_point};

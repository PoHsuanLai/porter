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
//! - [`Mounter`]: mounts through the system's `fusermount3`, run as a separate process (fuser
//!   with no default features links no libfuse). A stale mount at the path, left by a crash, is
//!   unmounted first (`fusermount3 -u -z`). No `/dev/fuse`, no `fusermount3`, or a mount that
//!   fails is a [`MountError`], and nothing else is touched. [`Mounted`] unmounts when dropped.

mod fs;
mod mount;

pub use fs::{Attr, DirEntry, DriveFs, Space, errno_of};
pub use mount::{MountError, Mounted, Mounter, is_mount_point};

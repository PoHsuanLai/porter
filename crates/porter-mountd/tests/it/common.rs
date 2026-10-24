//! A fake mount step: each mount is a pipe, whose writing end the fake keeps so a test can prove
//! the fd the caller got is the one the step made.

use porter_mountd::server::{At, Mounts};
use porter_mountd::wire::Access;
use std::collections::HashMap;
use std::io::{self, PipeWriter};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct FakeMounts {
    /// What is at each path (nothing when absent).
    pub table: Mutex<HashMap<PathBuf, At>>,
    /// The writing end of each mount's pipe, by path.
    pub writers: Mutex<HashMap<PathBuf, PipeWriter>>,
    /// Every unmount asked for, in order.
    pub unmounted: Mutex<Vec<PathBuf>>,
    /// The access of each mount made, by path.
    pub access: Mutex<HashMap<PathBuf, Access>>,
    /// Whether the mount step fails.
    pub fail: bool,
}

impl FakeMounts {
    pub fn with(path: PathBuf, at: At) -> Self {
        let fake = Self::default();
        fake.table.lock().expect("table").insert(path, at);
        fake
    }
}

impl Mounts for FakeMounts {
    fn at(&self, path: &Path) -> At {
        self.table
            .lock()
            .expect("table")
            .get(path)
            .cloned()
            .unwrap_or(At::Nothing)
    }

    fn mount(&self, path: &Path, access: Access) -> io::Result<OwnedFd> {
        self.access
            .lock()
            .expect("access")
            .insert(path.to_path_buf(), access);
        if self.fail {
            return Err(io::Error::other(
                "fusermount3: mount failed: Operation not permitted",
            ));
        }
        let (reader, writer) = io::pipe()?;
        self.writers
            .lock()
            .expect("writers")
            .insert(path.to_path_buf(), writer);
        self.table
            .lock()
            .expect("table")
            .insert(path.to_path_buf(), At::Ours { live: true });
        Ok(OwnedFd::from(reader))
    }

    fn unmount(&self, path: &Path) -> io::Result<()> {
        self.unmounted
            .lock()
            .expect("unmounted")
            .push(path.to_path_buf());
        self.table.lock().expect("table").remove(path);
        Ok(())
    }
}

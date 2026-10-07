//! The bearer token of an attached engine (vLLM's `--api-key`), read from a file at each connect.
//! The file is the person's own and private to them: a file any other user could read is refused,
//! and so is one another user owns. The token is held in [`Secret`] (whose `Debug` says nothing)
//! and goes nowhere but the `Authorization` header: no log line, error or `Debug` output of this
//! module holds it.

use model_http::Secret;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The longest key kept, in bytes: a token is a few dozen.
const LONGEST: u64 = 4096;

/// Why a key file was not used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyFileProblem {
    /// There is no file there.
    #[error("the key file is missing")]
    Missing,
    /// It could not be opened or read.
    #[error("the key file cannot be read: {0:?}")]
    Unreadable(std::io::ErrorKind),
    /// It is not a regular file.
    #[error("the key file is not a regular file")]
    NotAFile,
    /// Group or others have some access to it.
    #[error("the key file is readable by others (mode {mode:04o}); it must be 0600")]
    NotPrivate {
        /// Its permission bits.
        mode: u32,
    },
    /// Another user owns it.
    #[error("the key file belongs to another user")]
    NotYours,
    /// It holds nothing, or more than a token.
    #[error("the key file holds no usable token")]
    Unusable,
}

/// A key file's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFile(PathBuf);

impl KeyFile {
    /// The key file at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self(path)
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The token in the file now. It is opened once and looked at through that handle, so the
    /// file that was checked is the file that is read; a symlink is followed to the file it names
    /// (and that file is checked).
    pub fn read(&self) -> Result<Secret, KeyFileProblem> {
        let mut file = std::fs::File::open(&self.0).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => KeyFileProblem::Missing,
            kind => KeyFileProblem::Unreadable(kind),
        })?;
        let meta = file
            .metadata()
            .map_err(|e| KeyFileProblem::Unreadable(e.kind()))?;
        if !meta.is_file() {
            return Err(KeyFileProblem::NotAFile);
        }
        let mode = meta.mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(KeyFileProblem::NotPrivate { mode });
        }
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(KeyFileProblem::NotYours);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(LONGEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| KeyFileProblem::Unreadable(e.kind()))?;
        let text = String::from_utf8(bytes).map_err(|_| KeyFileProblem::Unusable)?;
        let token = text.trim();
        let fits = !token.is_empty()
            && u64::try_from(token.len()).is_ok_and(|len| len <= LONGEST)
            && token.chars().all(|c| c.is_ascii_graphic());
        match fits {
            true => Ok(Secret(token.to_owned())),
            false => Err(KeyFileProblem::Unusable),
        }
    }
}

#[cfg(test)]
mod tests;

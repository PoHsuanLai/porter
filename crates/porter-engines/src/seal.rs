//! The tree seal: one SHA-256 over every file and link of a directory tree. A person-owned
//! install is trusted only while its tree still has the seal the manifest pins for it, so the
//! same walk and the same lines are written by `tools/seal.py`, which pinned them: change this
//! text and every pinned seal changes with it.
//!
//! The text hashed is one line per file or link, sorted by the path (as bytes), each ending in a
//! newline: `F <x or -> <size> <sha256 of the content> <path>` for a file (`x` when any execute
//! bit is set), `L <target> <path>` for a symbolic link. Paths are relative to the root and use
//! `/`. Directories are not listed. The owner and the exact mode bits are not part of it, so
//! an extraction by any user and any umask seals the same.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A tree's seal as 64 lower-case hex digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSeal(String);

impl TreeSeal {
    /// The hex text.
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    /// Whether this seal is the pinned `hex`.
    pub fn is(&self, hex: &str) -> bool {
        self.0 == hex
    }
}

/// Why a tree could not be sealed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// A file or directory could not be read.
    #[error("cannot read {path}: {kind}")]
    Io {
        /// What was being read.
        path: PathBuf,
        /// What the system said.
        kind: std::io::ErrorKind,
    },
    /// Something that is not a file, a directory or a link (a socket, a device).
    #[error("{0} is not a file, a directory or a link")]
    Unsupported(PathBuf),
    /// A name or a link target that is not text.
    #[error("{0} has a name that is not text")]
    NotText(PathBuf),
}

fn io(path: &Path, error: &std::io::Error) -> SealError {
    SealError::Io {
        path: path.to_path_buf(),
        kind: error.kind(),
    }
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _written = write!(out, "{byte:02x}");
        out
    })
}

/// The size and SHA-256 of a file's content.
///
/// # Errors
/// [`SealError::Io`] when the file cannot be read.
pub fn file_digest(path: &Path) -> Result<(u64, String), SealError> {
    let mut file = std::fs::File::open(path).map_err(|e| io(path, &e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|e| io(path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

/// One entry of the tree as the line it makes and the path that sorts it.
fn line_of(root: &Path, path: &Path) -> Result<Option<(String, String)>, SealError> {
    let rel = path
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .ok_or_else(|| SealError::NotText(path.to_path_buf()))?
        .to_owned();
    let meta = std::fs::symlink_metadata(path).map_err(|e| io(path, &e))?;
    let kind = meta.file_type();
    if kind.is_dir() {
        return Ok(None);
    }
    let line = if kind.is_symlink() {
        let target = std::fs::read_link(path).map_err(|e| io(path, &e))?;
        let target = target
            .to_str()
            .ok_or_else(|| SealError::NotText(path.to_path_buf()))?;
        format!("L {target} {rel}\n")
    } else if kind.is_file() {
        let mode = if meta.permissions().mode() & 0o111 == 0 {
            '-'
        } else {
            'x'
        };
        let (size, digest) = file_digest(path)?;
        format!("F {mode} {size} {digest} {rel}\n")
    } else {
        return Err(SealError::Unsupported(path.to_path_buf()));
    };
    Ok(Some((rel, line)))
}

/// The seal of the tree at `root`.
///
/// # Errors
/// See [`SealError`].
pub fn seal_of(root: &Path) -> Result<TreeSeal, SealError> {
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| io(&dir, &e))? {
            let path = entry.map_err(|e| io(&dir, &e))?.path();
            if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_dir()) {
                pending.push(path.clone());
            }
            lines.extend(line_of(root, &path)?);
        }
    }
    lines.sort();
    let mut hasher = Sha256::new();
    for (_, line) in &lines {
        hasher.update(line.as_bytes());
    }
    Ok(TreeSeal(hex(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    fn make(root: &Path, mode: u32, name: &str, body: &str) {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, body).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("mode");
    }

    fn sample(dir: &Scratch) -> PathBuf {
        let root = dir.path().join("tree");
        make(&root, 0o644, "a", "hello\n");
        make(&root, 0o755, "d/run", "#!/bin/sh\n");
        std::os::unix::fs::symlink("run", root.join("d/link")).expect("link");
        root
    }

    /// The digest `tools/seal.py`, which pins the manifest's seals, gives the same tree: the two
    /// must agree.
    #[test]
    fn a_tree_has_the_seal_the_pinning_tool_gives_it() {
        let dir = Scratch::new("seal-vector");
        let root = sample(&dir);
        assert_eq!(
            seal_of(&root).expect("seal").as_hex(),
            "ab8e8a27eed121708fa02d6eb4112de90120f73122f6e5f77071ed54cb5952e3"
        );
    }

    #[test]
    fn a_changed_added_or_removed_entry_changes_the_seal_and_an_exact_mode_does_not() {
        let dir = Scratch::new("seal-change");
        let root = sample(&dir);
        let before = seal_of(&root).expect("seal");
        std::fs::set_permissions(root.join("a"), std::fs::Permissions::from_mode(0o600))
            .expect("mode");
        std::fs::set_permissions(root.join("d/run"), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        assert_eq!(
            seal_of(&root).expect("seal"),
            before,
            "the exact mode is not sealed"
        );
        std::fs::set_permissions(root.join("d/run"), std::fs::Permissions::from_mode(0o644))
            .expect("mode");
        assert_ne!(seal_of(&root).expect("seal"), before, "the execute bit is");
        std::fs::set_permissions(root.join("d/run"), std::fs::Permissions::from_mode(0o755))
            .expect("mode");
        make(&root, 0o644, "a", "hello!\n");
        assert_ne!(seal_of(&root).expect("seal"), before, "a changed file");
        make(&root, 0o644, "a", "hello\n");
        assert_eq!(seal_of(&root).expect("seal"), before);
        make(&root, 0o644, "extra", "");
        assert_ne!(seal_of(&root).expect("seal"), before, "an added file");
        std::fs::remove_file(root.join("extra")).expect("remove");
        std::fs::remove_file(root.join("d/link")).expect("remove");
        assert_ne!(seal_of(&root).expect("seal"), before, "a removed link");
    }

    #[test]
    fn a_tree_that_is_not_there_is_an_error_not_a_seal() {
        let dir = Scratch::new("seal-gone");
        assert!(matches!(
            seal_of(&dir.path().join("gone")),
            Err(SealError::Io { .. })
        ));
    }
}

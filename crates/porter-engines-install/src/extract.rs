//! Unpacking a pinned `tar.gz` in process (the `tar` and `flate2` crates), and settling the modes
//! of what it made. Only an archive whose SHA-256 is the pinned one is ever unpacked here, and
//! even then nothing in it may land outside the directory it is unpacked into: an absolute path,
//! a `..`, a path through a link, a link that points out, a device or a pipe is refused.

use flate2::read::MultiGzDecoder;
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Entry, EntryType};

/// Why an archive was not unpacked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExtractError {
    /// The archive could not be read: missing, not gzip, not tar, or cut short.
    #[error("the archive could not be unpacked")]
    Failed,
    /// An entry would land outside the directory, or is a kind of file an engine never ships (a
    /// device or a pipe). Nothing after it is unpacked; what came before it stays, inside.
    #[error("the archive has an entry that is not allowed: {0}")]
    Refused(String),
    /// The disk refused.
    #[error("the disk refused the unpacking: {0}")]
    Disk(std::io::ErrorKind),
}

/// An error from reading or writing an entry: damaged data is the archive's fault, the rest the
/// disk's.
fn of_io(error: &std::io::Error) -> ExtractError {
    use std::io::ErrorKind::{InvalidData, InvalidInput, UnexpectedEof};
    match error.kind() {
        InvalidData | InvalidInput | UnexpectedEof => ExtractError::Failed,
        kind => ExtractError::Disk(kind),
    }
}

/// Unpacks `archive` into the directory `into`, which must exist, without its top directory.
/// Owners are not kept, and modes only as far as their 0777 bits: [`settle_modes`] sets them
/// afterwards.
///
/// # Errors
/// See [`ExtractError`].
pub fn unpack(archive: &Path, into: &Path) -> Result<(), ExtractError> {
    let file = std::fs::File::open(archive).map_err(|_| ExtractError::Failed)?;
    let mut archive = Archive::new(MultiGzDecoder::new(BufReader::new(file)));
    archive.set_preserve_ownerships(false);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    for entry in archive.entries().map_err(|_| ExtractError::Failed)? {
        let mut entry = entry.map_err(|e| of_io(&e))?;
        place(&mut entry, into)?;
    }
    Ok(())
}

/// Writes one entry under `into`, or refuses it.
fn place<R: std::io::Read>(entry: &mut Entry<'_, R>, into: &Path) -> Result<(), ExtractError> {
    let named = entry.path().map_err(|_| ExtractError::Failed)?.into_owned();
    let refused = || ExtractError::Refused(named.display().to_string());
    let kind = entry.header().entry_type();
    if matches!(
        kind,
        EntryType::XGlobalHeader
            | EntryType::XHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink
    ) {
        return Ok(());
    }
    // The top directory itself has nowhere to go.
    let Some(relative) = stripped(&named).ok_or_else(refused)? else {
        return Ok(());
    };
    no_link_on_the_way(into, &relative).ok_or_else(refused)?;
    let at = into.join(&relative);
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).map_err(|e| of_io(&e))?;
    }
    match kind {
        EntryType::Regular
        | EntryType::Continuous
        | EntryType::GNUSparse
        | EntryType::Directory => {}
        EntryType::Symlink => {
            let target = entry
                .link_name()
                .map_err(|_| ExtractError::Failed)?
                .ok_or_else(refused)?;
            if !points_inside(&relative, &target) {
                return Err(refused());
            }
        }
        EntryType::Link => {
            let source = entry
                .link_name()
                .map_err(|_| ExtractError::Failed)?
                .ok_or_else(refused)?;
            let source = stripped(&source).ok_or_else(refused)?.ok_or_else(refused)?;
            no_link_on_the_way(into, &source).ok_or_else(refused)?;
            if std::fs::symlink_metadata(&at).is_ok_and(|meta| !meta.is_dir()) {
                std::fs::remove_file(&at).map_err(|e| of_io(&e))?;
            }
            return std::fs::hard_link(into.join(source), &at).map_err(|e| of_io(&e));
        }
        _ => return Err(refused()),
    }
    // A file or link already at `at` is replaced, never written through; a link where a
    // directory is wanted is an error.
    entry.unpack(&at).map(|_| ()).map_err(|e| of_io(&e))
}

/// `path` without its top directory: `None` when it is absolute or has a `..` (refused),
/// `Some(None)` when nothing is left (skipped). A `.` is no component.
fn stripped(path: &Path) -> Option<Option<PathBuf>> {
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => parts.push(name),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    let rest: PathBuf = parts.into_iter().skip(1).collect();
    Some((!rest.as_os_str().is_empty()).then_some(rest))
}

/// `Some` when no directory between `into` and `relative` (both ends left out) is a link, so a
/// write to `relative` cannot be carried outside by a link an earlier entry made.
fn no_link_on_the_way(into: &Path, relative: &Path) -> Option<()> {
    let mut at = into.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => return None,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Some(())
}

/// Whether a link at `relative` pointing at `target` stays inside: `target` is relative, and its
/// `..` come first and climb no higher than the top. The directories it climbs are real ones
/// (see [`no_link_on_the_way`]), and every link it later passes through was held to this rule.
fn points_inside(relative: &Path, target: &Path) -> bool {
    let mut depth = relative.components().count().saturating_sub(1);
    let mut descended = false;
    for part in target.components() {
        match part {
            Component::Normal(_) => descended = true,
            Component::CurDir => {}
            Component::ParentDir if !descended && depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
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
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};
    use test_kit::Scratch;

    /// One entry of a test archive, written as is: the name is not checked, so an archive can
    /// hold what a well-behaved `tar` never writes.
    struct Raw<'a> {
        name: &'a str,
        kind: EntryType,
        link: Option<&'a str>,
        body: &'a [u8],
    }

    fn file<'a>(name: &'a str, body: &'a [u8]) -> Raw<'a> {
        Raw {
            name,
            kind: EntryType::Regular,
            link: None,
            body,
        }
    }

    fn link<'a>(name: &'a str, kind: EntryType, to: &'a str) -> Raw<'a> {
        Raw {
            name,
            kind,
            link: Some(to),
            body: b"",
        }
    }

    fn gzip(builder: Builder<GzEncoder<Vec<u8>>>) -> Vec<u8> {
        builder.into_inner().expect("tar").finish().expect("gzip")
    }

    /// Unpacks `entries` into a fresh `into` beside a file `outside`.
    fn try_unpack(name: &str, entries: &[Raw<'_>]) -> (Scratch, Result<(), ExtractError>) {
        let dir = Scratch::new(name);
        std::fs::write(dir.path().join("outside"), "kept").expect("outside");
        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for raw in entries {
            let mut header = Header::new_gnu();
            header.as_old_mut().name[..raw.name.len()].copy_from_slice(raw.name.as_bytes());
            header.set_entry_type(raw.kind);
            header.set_size(raw.body.len() as u64);
            header.set_mode(0o644);
            if let Some(to) = raw.link {
                header.set_link_name(to).expect("link name");
            }
            header.set_cksum();
            builder.append(&header, raw.body).expect("entry");
        }
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(&archive, gzip(builder)).expect("archive");
        let into = dir.path().join("into");
        std::fs::create_dir(&into).expect("into");
        let result = unpack(&archive, &into);
        (dir, result)
    }

    /// Nothing beside `into` was written or changed.
    fn untouched(dir: &Scratch) {
        assert_eq!(
            std::fs::read_to_string(dir.path().join("outside")).expect("outside"),
            "kept"
        );
        assert!(!dir.path().join("escaped").exists());
    }

    fn refused(result: &Result<(), ExtractError>) -> bool {
        matches!(result, Err(ExtractError::Refused(_)))
    }

    #[test]
    fn an_archive_is_unpacked_without_its_top_directory_and_modes_are_settled() {
        let dir = Scratch::new("extract");
        let top = dir.path().join("top");
        std::fs::create_dir_all(top.join("sub")).expect("dir");
        std::fs::write(top.join("run"), "#!/bin/sh\n").expect("run");
        std::fs::write(top.join("sub/data"), "d").expect("data");
        std::os::unix::fs::symlink("run", top.join("link")).expect("link");
        std::os::unix::fs::symlink("../run", top.join("sub/up")).expect("up");
        std::fs::set_permissions(top.join("run"), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        builder.follow_symlinks(false);
        builder.append_dir_all("top", &top).expect("tree");
        let archive = dir.path().join("a.tar.gz");
        std::fs::write(&archive, gzip(builder)).expect("archive");
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
        assert_eq!(
            std::fs::read_to_string(into.join("sub/up")).expect("through the link"),
            "#!/bin/sh\n"
        );
        assert!(!into.join("top").exists());
        assert_eq!(
            unpack(&dir.path().join("gone.tar.gz"), &into),
            Err(ExtractError::Failed)
        );
        std::fs::write(dir.path().join("plain"), "not gzip").expect("plain");
        assert_eq!(
            unpack(&dir.path().join("plain"), &into),
            Err(ExtractError::Failed)
        );
    }

    #[test]
    fn a_hard_link_inside_is_made() {
        let (dir, result) = try_unpack(
            "extract-hard",
            &[
                file("top/a", b"same"),
                link("top/b", EntryType::Link, "top/a"),
            ],
        );
        result.expect("unpack");
        let b = dir.path().join("into/b");
        assert_eq!(std::fs::read_to_string(b).expect("b"), "same");
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let name = "/porter-extract-test-escaped";
        let (dir, result) = try_unpack("extract-absolute", &[file(name, b"x")]);
        assert!(refused(&result), "{result:?}");
        assert!(!Path::new(name).exists());
        untouched(&dir);
    }

    #[test]
    fn a_parent_component_is_refused_even_when_it_would_stay_inside() {
        for name in ["top/../../escaped", "top/../escaped", "top/sub/../x"] {
            let (dir, result) = try_unpack("extract-dotdot", &[file(name, b"x")]);
            assert!(refused(&result), "{name}: {result:?}");
            untouched(&dir);
        }
    }

    #[test]
    fn a_link_that_points_out_is_refused() {
        for to in [
            "/etc",
            "..",
            "../escaped",
            "sub/../../escaped",
            "a/../..",
            "./../x",
        ] {
            let (dir, result) = try_unpack(
                "extract-link-out",
                &[link("top/out", EntryType::Symlink, to)],
            );
            assert!(refused(&result), "{to}: {result:?}");
            assert!(!dir.path().join("into/out").exists(), "{to}");
            untouched(&dir);
        }
    }

    #[test]
    fn a_link_one_level_down_may_climb_one_level_and_no_more() {
        let (dir, result) = try_unpack(
            "extract-link-up",
            &[
                file("top/lib/real.so", b"lib"),
                link("top/bin/lib.so", EntryType::Symlink, "../lib/real.so"),
            ],
        );
        result.expect("unpack");
        let through = dir.path().join("into/bin/lib.so");
        assert_eq!(std::fs::read_to_string(through).expect("lib"), "lib");
        let (dir, result) = try_unpack(
            "extract-link-up-two",
            &[link("top/bin/out", EntryType::Symlink, "../../escaped")],
        );
        assert!(refused(&result), "{result:?}");
        untouched(&dir);
    }

    #[test]
    fn a_write_through_a_link_is_refused() {
        // `dir` points at the tree itself, so `dir/x` would land inside; it is refused anyway,
        // because which way a link points is not checked again when something goes through it.
        let (dir, result) = try_unpack(
            "extract-through",
            &[
                link("top/dir", EntryType::Symlink, "."),
                file("top/dir/x", b"x"),
            ],
        );
        assert!(refused(&result), "{result:?}");
        assert!(!dir.path().join("into/x").exists());
        untouched(&dir);
    }

    #[test]
    fn a_file_over_a_link_replaces_the_link() {
        let (dir, result) = try_unpack(
            "extract-over-link",
            &[
                link("top/x", EntryType::Symlink, "y"),
                file("top/x", b"plain"),
            ],
        );
        result.expect("unpack");
        let x = dir.path().join("into/x");
        assert!(std::fs::symlink_metadata(&x).expect("x").is_file());
        assert!(!dir.path().join("into/y").exists());
    }

    #[test]
    fn a_hard_link_to_outside_is_refused() {
        for to in ["/etc/passwd", "top/../../outside", "outside"] {
            let (dir, result) =
                try_unpack("extract-hard-out", &[link("top/h", EntryType::Link, to)]);
            assert!(refused(&result), "{to}: {result:?}");
            assert!(!dir.path().join("into/h").exists(), "{to}");
            untouched(&dir);
        }
    }

    #[test]
    fn devices_and_pipes_are_refused() {
        for kind in [EntryType::Fifo, EntryType::Char, EntryType::Block] {
            let entry = Raw {
                name: "top/dev",
                kind,
                link: None,
                body: b"",
            };
            let (dir, result) = try_unpack("extract-special", &[entry]);
            assert!(refused(&result), "{kind:?}: {result:?}");
            assert!(!dir.path().join("into/dev").exists());
        }
    }
}

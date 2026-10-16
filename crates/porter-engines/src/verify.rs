//! The check made before an engine of a person's own install starts. An install the person owns
//! is a tree anything running as them could change, so it is trusted only while it is the tree
//! the manifest pins:
//!
//! - an archive install (llama.cpp): the whole tree's seal must be the one compiled into the
//!   manifest row, so a changed or added file anywhere in it refuses the start;
//! - a Python environment (vLLM): its Python's tree must have the pinned seal, and the files that
//!   can run code at start (the interpreter's launchers, `pyvenv.cfg`, `.pth` files, the
//!   `sitecustomize` hooks and the engine's own package) must be as the install recorded them.
//!   The whole environment is checked against the record afterwards, in the background
//!   ([`Verifier::full`]), because it is gigabytes.
//!
//! An install for everyone is root's and is not checked here: the system path rule already
//! holds it.

use crate::id::EngineId;
use crate::layout::{InstallDir, Record, Roots, Scope};
use crate::manifest::{Delivery, Manifest, PythonEnv, Row};
use crate::seal::{SealError, file_digest, seal_of};
use std::collections::BTreeMap;
use std::path::Path;

/// What the check says of a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not a person's own install of ours: nothing to check here.
    NotChecked,
    /// It is the pinned install.
    Verified,
    /// It is not: it was changed since it was installed, or nothing pins it.
    Changed(EngineId),
}

/// Checks programs against the manifest.
#[derive(Debug, Clone)]
pub struct Verifier {
    roots: Roots,
    manifest: Manifest,
}

impl Verifier {
    /// A verifier of installs under `roots`, pinned by `manifest`.
    pub fn new(roots: Roots, manifest: Manifest) -> Self {
        Self { roots, manifest }
    }

    /// The row and directory of the person's install `program` lies in; `Err(verdict)` when it
    /// is not one to check or cannot be matched to the manifest.
    fn install(&self, program: &Path) -> Result<(&'static Row, InstallDir, Record), Verdict> {
        let Some((scope, engine, name)) = self.roots.locate(program) else {
            return Err(Verdict::NotChecked);
        };
        if scope == Scope::AllUsers {
            return Err(Verdict::NotChecked);
        }
        let changed = Verdict::Changed(engine);
        let row = self.manifest.row_in_dir(engine, &name).ok_or(changed)?;
        let dir = self.roots.dir(scope, row);
        let record = Record::read(&dir.record()).map_err(|_| changed)?;
        let matches = record.engine == engine.slug()
            && record.version == row.version
            && record.backend == row.backend.slug();
        matches.then_some((row, dir, record)).ok_or(changed)
    }

    /// The check before a start.
    pub fn check(&self, program: &Path) -> Verdict {
        let (row, dir, record) = match self.install(program) {
            Ok(found) => found,
            Err(verdict) => return verdict,
        };
        let changed = Verdict::Changed(row.engine);
        let sound = match row.delivery {
            Delivery::Archives(_) => row
                .tree_seal
                .zip(seal_of(&dir.tree()).ok())
                .is_some_and(|(pinned, found)| found.is(pinned)),
            Delivery::Python(env) => entry_set_holds(&dir, &env, &record),
        };
        if sound { Verdict::Verified } else { changed }
    }

    /// The slow check of what [`Self::check`] left: a Python environment's whole `venv/`
    /// against the record. Every other install is whole after `check`.
    pub fn full(&self, program: &Path) -> Verdict {
        let (row, dir, record) = match self.install(program) {
            Ok(found) => found,
            Err(verdict) => return verdict,
        };
        match row.delivery {
            Delivery::Archives(_) => Verdict::Verified,
            Delivery::Python(_) => {
                let held = record.venv_seal.zip(seal_of(&dir.tree().join("venv")).ok());
                if held.is_some_and(|(recorded, found)| found.is(&recorded)) {
                    Verdict::Verified
                } else {
                    Verdict::Changed(row.engine)
                }
            }
        }
    }
}

/// Whether the Python is the pinned one and the files that run code at start are as recorded.
fn entry_set_holds(dir: &InstallDir, env: &PythonEnv, record: &Record) -> bool {
    let python_pinned =
        seal_of(&dir.tree().join("python")).is_ok_and(|seal| seal.is(env.python_seal));
    python_pinned
        && entry_digests(&dir.tree(), env.package).is_ok_and(|found| found == record.entries)
}

/// Whether `rel` (relative to the tree, with `/`) is a file that can run code when the
/// environment's Python starts or that is the engine's own.
fn is_entry(rel: &str, package: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    rel.starts_with("venv/bin/")
        || name == "pyvenv.cfg"
        || name == "sitecustomize.py"
        || name == "usercustomize.py"
        || name.ends_with(".pth")
        || rel.contains(&format!("/site-packages/{package}/"))
}

/// The digest of each entry file of the environment under `tree`, by its path under `tree`: a
/// file's SHA-256, or `link:<target>` for a link. A file added or removed changes the map.
///
/// # Errors
/// See [`SealError`].
pub fn entry_digests(tree: &Path, package: &str) -> Result<BTreeMap<String, String>, SealError> {
    let mut found = BTreeMap::new();
    let mut pending = vec![tree.join("venv")];
    while let Some(dir) = pending.pop() {
        let io = |e: &std::io::Error| SealError::Io {
            path: dir.clone(),
            kind: e.kind(),
        };
        for entry in std::fs::read_dir(&dir).map_err(|e| io(&e))? {
            let path = entry.map_err(|e| io(&e))?.path();
            let kind = std::fs::symlink_metadata(&path)
                .map_err(|e| io(&e))?
                .file_type();
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(tree)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| SealError::NotText(path.clone()))?
                .to_owned();
            if !is_entry(&rel, package) {
                continue;
            }
            let digest = if kind.is_symlink() {
                let target = std::fs::read_link(&path).map_err(|e| io(&e))?;
                format!("link:{}", target.to_string_lossy())
            } else {
                file_digest(&path)?.1
            };
            found.insert(rel, digest);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Arch, Backend};
    use crate::manifest::Artifact;
    use porter_fake::Scratch;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    static EMPTY: [Artifact; 0] = [];

    /// A one-file archive install and the seal of its tree, made here.
    fn archive_row(seal: &'static str) -> &'static Row {
        Box::leak(Box::new(Row {
            engine: EngineId::LlamaCpp,
            arch: Arch::X86_64,
            backend: Backend::Cpu,
            version: "b1",
            min_nvidia_driver: None,
            delivery: Delivery::Archives(&EMPTY),
            program: "llama-server",
            tree_seal: Some(seal),
        }))
    }

    fn write(path: &Path, body: &str, mode: u32) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, body).expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("mode");
    }

    fn installed(dir: &Scratch, body: &str) -> (Verifier, PathBuf, &'static Row) {
        let roots = Roots::new(dir.path().join("system"), dir.path().join("person"));
        let install = roots
            .engine_dir(Scope::JustMe, EngineId::LlamaCpp)
            .join("b1-cpu");
        let program = install.join("engine/llama-server");
        write(&program, body, 0o755);
        let seal = seal_of(&install.join("engine")).expect("seal");
        let row = archive_row(Box::leak(seal.as_hex().to_owned().into_boxed_str()));
        let record = Record {
            engine: "llama_cpp".into(),
            version: "b1".into(),
            backend: "cpu".into(),
            seal: seal.as_hex().to_owned(),
            entries: BTreeMap::new(),
            venv_seal: None,
        };
        record
            .write(&install.join("installed.json"))
            .expect("record");
        let manifest = Manifest::of(Box::leak(vec![*row].into_boxed_slice()));
        (Verifier::new(roots, manifest), program, row)
    }

    #[test]
    fn a_pinned_install_passes_and_any_change_to_its_tree_refuses_it() {
        let dir = Scratch::new("verify-archive");
        let (verifier, program, _) = installed(&dir, "#!/bin/sh\n");
        assert_eq!(verifier.check(&program), Verdict::Verified);
        write(&program.with_file_name("libextra.so"), "x", 0o644);
        assert_eq!(
            verifier.check(&program),
            Verdict::Changed(EngineId::LlamaCpp)
        );
        std::fs::remove_file(program.with_file_name("libextra.so")).expect("remove");
        assert_eq!(verifier.check(&program), Verdict::Verified);
        write(&program, "#!/bin/sh\nevil\n", 0o755);
        assert_eq!(
            verifier.check(&program),
            Verdict::Changed(EngineId::LlamaCpp)
        );
    }

    #[test]
    fn a_seal_nothing_pins_a_missing_record_and_an_unlisted_directory_refuse() {
        let dir = Scratch::new("verify-pins");
        let (verifier, program, _) = installed(&dir, "#!/bin/sh\n");
        let record = program
            .parent()
            .expect("engine")
            .parent()
            .expect("dir")
            .join("installed.json");
        std::fs::remove_file(&record).expect("remove");
        assert_eq!(
            verifier.check(&program),
            Verdict::Changed(EngineId::LlamaCpp)
        );
        let other = program.to_string_lossy().replace("b1-cpu", "b0-cpu");
        assert_eq!(
            verifier.check(Path::new(&other)),
            Verdict::Changed(EngineId::LlamaCpp)
        );
    }

    #[test]
    fn a_program_that_is_not_a_persons_install_is_not_checked() {
        let dir = Scratch::new("verify-other");
        let (verifier, _, _) = installed(&dir, "#!/bin/sh\n");
        assert_eq!(
            verifier.check(Path::new("/usr/bin/llama-server")),
            Verdict::NotChecked
        );
        let system = dir
            .path()
            .join("system/llama_cpp/b1-cpu/engine/llama-server");
        assert_eq!(verifier.check(&system), Verdict::NotChecked);
    }

    #[test]
    fn the_entry_set_has_the_launchers_the_hooks_and_the_engines_own_files_and_sees_additions() {
        let dir = Scratch::new("verify-entries");
        let tree = dir.path().join("engine");
        write(&tree.join("venv/pyvenv.cfg"), "home = x\n", 0o644);
        write(&tree.join("venv/bin/activate"), "a", 0o644);
        write(
            &tree.join("venv/lib/python3.12/site-packages/vllm/__init__.py"),
            "v",
            0o644,
        );
        write(
            &tree.join("venv/lib/python3.12/site-packages/torch/__init__.py"),
            "t",
            0o644,
        );
        write(
            &tree.join("venv/lib/python3.12/site-packages/a.pth"),
            "import os\n",
            0o644,
        );
        std::os::unix::fs::symlink("../../python/bin/python3", tree.join("venv/bin/python"))
            .expect("link");
        let before = entry_digests(&tree, "vllm").expect("digests");
        let names: Vec<&str> = before.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "venv/bin/activate",
                "venv/bin/python",
                "venv/lib/python3.12/site-packages/a.pth",
                "venv/lib/python3.12/site-packages/vllm/__init__.py",
                "venv/pyvenv.cfg",
            ]
        );
        assert_eq!(before["venv/bin/python"], "link:../../python/bin/python3");
        write(
            &tree.join("venv/lib/python3.12/site-packages/torch/__init__.py"),
            "t2",
            0o644,
        );
        assert_eq!(
            entry_digests(&tree, "vllm").expect("digests"),
            before,
            "a library is not an entry"
        );
        write(
            &tree.join("venv/lib/python3.12/site-packages/b.pth"),
            "x",
            0o644,
        );
        assert_ne!(
            entry_digests(&tree, "vllm").expect("digests"),
            before,
            "an added hook is"
        );
    }
}

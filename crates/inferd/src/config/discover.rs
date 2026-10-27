//! Engine programs found at fixed system paths and in the installs of ours. This is not the
//! environment: no `$PATH`, no variable. A system path counts only when it is a regular
//! executable file that root holds, so a person (or a program running as them) cannot put a
//! program there. An install for everyone is under such a path; one for the person is under their
//! own data directory and is checked against the manifest's pins each time it is started (see
//! [`crate::hosts::LaunchGuard`]). An explicit path in `inferd.toml` always wins and is never
//! checked here.

use porter_daemon::root_held;
use porter_engines::{EngineId, Finder, Place, Roots};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Where a distro package puts llama.cpp's server, first choice first.
const LLAMA_SERVER_PATHS: [&str; 2] = ["/usr/bin/llama-server", "/usr/local/bin/llama-server"];

/// Where to look for the programs a package installs, and how to tell a path is trustworthy.
#[derive(Debug, Clone)]
pub struct ProgramSearch {
    llama_server: Vec<PathBuf>,
    held: fn(&Path) -> bool,
    installs: Option<Finder>,
}

impl ProgramSearch {
    /// The system's fixed paths, held by root.
    pub fn system() -> Self {
        Self::in_paths(
            LLAMA_SERVER_PATHS.iter().map(PathBuf::from).collect(),
            system_held,
        )
    }

    /// These paths for llama.cpp's server, trusted when `held` says so (a test's own list).
    pub fn in_paths(llama_server: Vec<PathBuf>, held: fn(&Path) -> bool) -> Self {
        Self {
            llama_server,
            held,
            installs: None,
        }
    }

    /// The same, also looking for the engines installed under `roots` and for the packages of
    /// each at their usual paths.
    #[must_use]
    pub fn with_installs(self, finder: Finder) -> Self {
        Self {
            installs: Some(finder),
            ..self
        }
    }

    /// The system's fixed paths and the installs under `roots`.
    pub fn system_with_installs(roots: Roots) -> Self {
        Self::system().with_installs(Finder::new(roots, root_held))
    }

    /// The program the `llama_server` setting means: the explicit path when there is one, else
    /// the first fixed path that is trusted.
    pub fn llama_server(&self, explicit: Option<&Path>) -> Option<PathBuf> {
        explicit
            .map(Path::to_path_buf)
            .or_else(|| {
                self.llama_server
                    .iter()
                    .find(|path| (self.held)(path))
                    .cloned()
            })
            .or_else(|| self.found(EngineId::LlamaCpp).map(|found| found.program))
    }

    /// The Python of vLLM's environment: the explicit path when there is one, else the Python of
    /// an install of ours, else the interpreter a packaged `vllm` command names.
    pub fn vllm_python(&self, explicit: Option<&Path>) -> Option<PathBuf> {
        explicit.map(Path::to_path_buf).or_else(|| {
            let found = self.found(EngineId::Vllm)?;
            match found.place {
                Place::Package => interpreter_of(&found.program, self.held),
                Place::System | Place::Person | Place::Pointed => Some(found.program),
            }
        })
    }

    fn found(&self, engine: EngineId) -> Option<porter_engines::Found> {
        self.installs.as_ref()?.find(engine)
    }
}

/// The interpreter a script names on its first line (`#!/usr/bin/python3`), when it is an
/// absolute path that `held` trusts. `#!/usr/bin/env python3` names none.
fn interpreter_of(script: &Path, held: fn(&Path) -> bool) -> Option<PathBuf> {
    let text = std::fs::read(script).ok()?;
    let first = text.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(first).ok()?.strip_prefix("#!")?.trim();
    let path = PathBuf::from(line.split_whitespace().next()?);
    (path.is_absolute() && !path.ends_with("env") && held(&path)).then_some(path)
}

/// A regular file with an execute bit that only root could have written.
fn system_held(path: &Path) -> bool {
    root_held(path)
        && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    fn file(dir: &Scratch, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, b"#!/bin/sh\n").expect("write");
        path
    }

    /// Stands in for root ownership: everything but a file named `theirs`.
    fn not_theirs(path: &Path) -> bool {
        path.exists() && path.file_name().is_some_and(|name| name != "theirs")
    }

    #[test]
    fn an_explicit_path_wins_and_the_list_is_not_looked_at() {
        let dir = Scratch::new("discover-explicit");
        let search = ProgramSearch::in_paths(vec![file(&dir, "first")], not_theirs);
        assert_eq!(
            search.llama_server(Some(Path::new("/mine/llama-server"))),
            Some(PathBuf::from("/mine/llama-server"))
        );
    }

    #[test]
    fn the_first_trusted_existing_file_of_the_list_is_used() {
        let dir = Scratch::new("discover-list");
        let (gone, theirs, first, second) = (
            dir.path().join("gone"),
            file(&dir, "theirs"),
            file(&dir, "first"),
            file(&dir, "second"),
        );
        let search = ProgramSearch::in_paths(
            vec![gone, theirs, first.clone(), second.clone()],
            not_theirs,
        );
        assert_eq!(search.llama_server(None), Some(first));
        let only_second =
            ProgramSearch::in_paths(vec![dir.path().join("gone"), second.clone()], not_theirs);
        assert_eq!(only_second.llama_server(None), Some(second));
    }

    #[test]
    fn a_packaged_commands_interpreter_is_its_first_line_when_that_is_held() {
        let dir = Scratch::new("discover-interpreter");
        let python = file(&dir, "python3");
        let script = |first: &str| {
            let path = dir.path().join("vllm");
            std::fs::write(&path, format!("{first}\nimport vllm\n")).expect("write");
            path
        };
        let named = script(&format!("#!{}", python.display()));
        assert_eq!(interpreter_of(&named, not_theirs), Some(python.clone()));
        assert_eq!(interpreter_of(&named, |_| false), None);
        assert_eq!(
            interpreter_of(&script("#!/usr/bin/env python3"), not_theirs),
            None
        );
        assert_eq!(interpreter_of(&script("#!python3"), not_theirs), None);
        assert_eq!(interpreter_of(&script("import vllm"), not_theirs), None);
        assert_eq!(interpreter_of(&dir.path().join("gone"), not_theirs), None);
    }

    #[test]
    fn a_list_with_nothing_trusted_finds_nothing() {
        let dir = Scratch::new("discover-none");
        let search = ProgramSearch::in_paths(
            vec![dir.path().join("gone"), file(&dir, "theirs")],
            not_theirs,
        );
        assert_eq!(search.llama_server(None), None);
    }

    #[test]
    fn the_system_check_refuses_a_file_a_person_wrote_and_a_file_that_is_not_executable() {
        let dir = Scratch::new("discover-system");
        let mine = file(&dir, "mine");
        assert!(!system_held(&mine), "the scratch directory is not root's");
        assert!(!system_held(dir.path()), "a directory is not a program");
        assert!(!system_held(&dir.path().join("gone")));
    }
}

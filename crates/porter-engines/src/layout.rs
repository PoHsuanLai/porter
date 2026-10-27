//! Where installed engines live, and the record each install leaves.
//!
//! One directory per engine under a root (`<root>/<engine>/<version>-<backend>/`). It holds the
//! unpacked `engine/` tree and `installed.json`, the record. An install is built in a sibling
//! `.<name>.partial/` and renamed into place whole, so a directory without a leading dot is a
//! finished install.

use crate::id::EngineId;
use crate::manifest::Row;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// For whom an engine is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everyone on this computer: root-owned, installed by the privileged helper.
    AllUsers,
    /// The person alone: in their own data directory, checked against the pins at each start.
    JustMe,
}

impl Scope {
    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            Self::AllUsers => "all_users",
            Self::JustMe => "just_me",
        }
    }

    /// The scope a word names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        [Self::AllUsers, Self::JustMe]
            .into_iter()
            .find(|scope| scope.slug() == text)
    }
}

/// The two roots engines are installed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    system: PathBuf,
    person: PathBuf,
}

impl Roots {
    /// Where an all-users install goes: fixed, root's.
    pub const SYSTEM: &'static str = "/usr/local/lib/porter/engines";

    /// These two roots (a test's own).
    pub fn new(system: PathBuf, person: PathBuf) -> Self {
        Self { system, person }
    }

    /// The standard roots: [`Self::SYSTEM`] and `<data_home>/porter/engines`.
    pub fn standard(data_home: &Path) -> Self {
        Self::new(
            PathBuf::from(Self::SYSTEM),
            data_home.join("porter").join("engines"),
        )
    }

    /// The root of `scope`.
    pub fn root(&self, scope: Scope) -> &Path {
        match scope {
            Scope::AllUsers => &self.system,
            Scope::JustMe => &self.person,
        }
    }

    /// The directory that holds every install of `engine` for `scope`.
    pub fn engine_dir(&self, scope: Scope, engine: EngineId) -> PathBuf {
        self.root(scope).join(engine.slug())
    }

    /// Where `row` installs for `scope`.
    pub fn dir(&self, scope: Scope, row: &Row) -> InstallDir {
        InstallDir(self.engine_dir(scope, row.engine).join(row.dir_name()))
    }

    /// The install a program path lies in: its scope, engine and directory name. `None` for a
    /// path that is not under an install of ours.
    pub fn locate(&self, program: &Path) -> Option<(Scope, EngineId, String)> {
        [Scope::AllUsers, Scope::JustMe]
            .into_iter()
            .find_map(|scope| {
                let rest = program.strip_prefix(self.root(scope)).ok()?;
                let mut parts = rest.components();
                let engine = EngineId::from_slug(parts.next()?.as_os_str().to_str()?)?;
                let dir = parts.next()?.as_os_str().to_str()?.to_owned();
                (!dir.starts_with('.')).then_some((scope, engine, dir))
            })
    }
}

/// One install's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallDir(PathBuf);

impl InstallDir {
    /// The install directory at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self(path)
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The unpacked tree.
    pub fn tree(&self) -> PathBuf {
        self.0.join("engine")
    }

    /// The record.
    pub fn record(&self) -> PathBuf {
        self.0.join("installed.json")
    }

    /// The program of `row`.
    pub fn program(&self, row: &Row) -> PathBuf {
        self.tree().join(row.program)
    }

    /// The directory the install is built in before it is renamed to this one.
    pub fn partial(&self) -> PathBuf {
        let name = self
            .0
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.0.with_file_name(format!(".{name}.partial"))
    }
}

/// What an install leaves beside its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The engine's slug.
    pub engine: String,
    /// The release.
    pub version: String,
    /// The backend's slug.
    pub backend: String,
    /// The seal the tree had when it was installed.
    pub seal: String,
    /// For a Python environment: the digest of each file of its entry set, by path under the
    /// tree. Empty for an archive install, whose whole tree is pinned by the manifest.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
    /// For a Python environment: the seal of its `venv/` directory.
    #[serde(default)]
    pub venv_seal: Option<String>,
}

/// Why a record could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// The file could not be read or written.
    #[error("the install record cannot be read or written: {0}")]
    Io(std::io::ErrorKind),
    /// The file is not a record.
    #[error("the install record is not readable")]
    Malformed,
}

impl Record {
    /// The record at `path`.
    ///
    /// # Errors
    /// See [`RecordError`].
    pub fn read(path: &Path) -> Result<Self, RecordError> {
        let text = std::fs::read(path).map_err(|e| RecordError::Io(e.kind()))?;
        serde_json::from_slice(&text).map_err(|_| RecordError::Malformed)
    }

    /// Writes the record at `path`: to a file beside it first, then renamed over it.
    ///
    /// # Errors
    /// See [`RecordError`].
    pub fn write(&self, path: &Path) -> Result<(), RecordError> {
        let text = serde_json::to_vec_pretty(self).map_err(|_| RecordError::Malformed)?;
        let staged = path.with_extension("json.new");
        std::fs::write(&staged, text).map_err(|e| RecordError::Io(e.kind()))?;
        std::fs::rename(&staged, path).map_err(|e| RecordError::Io(e.kind()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    #[test]
    fn a_program_path_is_located_in_its_scope_and_a_partial_directory_is_not_an_install() {
        let roots = Roots::new(
            "/sys/engines".into(),
            "/home/p/.local/share/porter/engines".into(),
        );
        let located = |path: &str| roots.locate(Path::new(path));
        assert_eq!(
            located("/sys/engines/llama_cpp/b1-vulkan/engine/llama-server"),
            Some((Scope::AllUsers, EngineId::LlamaCpp, "b1-vulkan".to_owned()))
        );
        assert_eq!(
            located("/home/p/.local/share/porter/engines/vllm/0.31.0-cuda/engine/venv/bin/python"),
            Some((Scope::JustMe, EngineId::Vllm, "0.31.0-cuda".to_owned()))
        );
        assert_eq!(
            located("/sys/engines/llama_cpp/.b1-vulkan.partial/engine/x"),
            None
        );
        assert_eq!(located("/sys/engines/other/b1/x"), None);
        assert_eq!(located("/usr/bin/llama-server"), None);
    }

    #[test]
    fn a_record_round_trips_and_a_missing_or_foreign_file_is_an_error() {
        let dir = Scratch::new("layout-record");
        let path = dir.path().join("installed.json");
        let record = Record {
            engine: "llama_cpp".into(),
            version: "b1".into(),
            backend: "cpu".into(),
            seal: "00".into(),
            entries: BTreeMap::from([("a".to_owned(), "11".to_owned())]),
            venv_seal: None,
        };
        record.write(&path).expect("write");
        assert_eq!(Record::read(&path), Ok(record));
        assert_eq!(
            Record::read(&dir.path().join("gone")),
            Err(RecordError::Io(std::io::ErrorKind::NotFound))
        );
        std::fs::write(&path, "[]").expect("write");
        assert_eq!(Record::read(&path), Err(RecordError::Malformed));
    }

    #[test]
    fn the_partial_directory_sits_beside_the_install() {
        let dir = InstallDir::at(PathBuf::from("/r/llama_cpp/b1-cpu"));
        assert_eq!(dir.partial(), PathBuf::from("/r/llama_cpp/.b1-cpu.partial"));
        assert_eq!(
            dir.record(),
            PathBuf::from("/r/llama_cpp/b1-cpu/installed.json")
        );
    }
}

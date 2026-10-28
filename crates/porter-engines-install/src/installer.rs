//! Installing a pinned row: space checked, files fetched, unpacked and sealed in a directory
//! beside the install, and renamed into place whole. Whoever runs it owns what it makes: the
//! person for an install of their own, root (through the helper) for everyone's.

use crate::extract::{ExtractError, settle_modes, unpack};
use crate::fetch::{FetchError, Fetcher};
use crate::progress::{CancelToken, Meter};
use crate::pyenv;
use crate::space::require_space;
use porter_engines::{
    Delivery, Failure, InstallDir, Record, Roots, Row, Scope, SealError, seal_of,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Why an install did not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InstallError {
    /// Not enough free space.
    #[error("not enough free space: needs {need} bytes, {free} are free")]
    Space {
        /// Bytes needed.
        need: u64,
        /// Bytes free.
        free: u64,
    },
    /// A download failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// An archive could not be unpacked.
    #[error(transparent)]
    Extract(#[from] ExtractError),
    /// The unpacked files are not the pinned ones.
    #[error("the unpacked files are not the pinned ones")]
    SealMismatch,
    /// A tree could not be read to be sealed.
    #[error(transparent)]
    Seal(#[from] SealError),
    /// The disk refused.
    #[error("the disk refused the install: {0}")]
    Disk(std::io::ErrorKind),
    /// The Python environment could not be built.
    #[error("the environment could not be built")]
    Environment,
    /// A stop was asked for.
    #[error("a stop was asked for")]
    Cancelled,
}

impl InstallError {
    /// What a person is told.
    pub fn failure(&self) -> Failure {
        let full = |kind: &std::io::ErrorKind| *kind == std::io::ErrorKind::StorageFull;
        match self {
            Self::Space { .. } => Failure::NoSpace,
            Self::Fetch(FetchError::Network(_)) => Failure::Offline,
            Self::Fetch(FetchError::Corrupt) | Self::SealMismatch => Failure::Corrupt,
            Self::Fetch(FetchError::Disk(kind))
            | Self::Extract(ExtractError::Disk(kind))
            | Self::Disk(kind)
                if full(kind) =>
            {
                Failure::NoSpace
            }
            Self::Fetch(FetchError::Cancelled) | Self::Cancelled => Failure::Cancelled,
            Self::Fetch(_)
            | Self::Extract(_)
            | Self::Seal(_)
            | Self::Disk(_)
            | Self::Environment => Failure::Other,
        }
    }

    /// Whether the downloaded files are worth keeping for the next try.
    fn keeps_downloads(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Environment
                | Self::Fetch(FetchError::Network(_) | FetchError::Cancelled)
        )
    }
}

fn disk(error: &std::io::Error) -> InstallError {
    InstallError::Disk(error.kind())
}

/// Runs blocking work off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, InstallError> + Send + 'static,
) -> Result<T, InstallError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or(Err(InstallError::Environment))
}

/// Installs rows under a set of roots.
#[derive(Debug, Clone)]
pub struct Installer {
    fetcher: Fetcher,
    roots: Roots,
}

impl Installer {
    /// An installer that downloads with `fetcher` and installs under `roots`.
    pub fn new(fetcher: Fetcher, roots: Roots) -> Self {
        Self { fetcher, roots }
    }

    /// The roots it installs under.
    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    /// Installs `row` for `scope`, counting its bytes in `meter` (made with the row's
    /// download size). An install that is there already is left as it is.
    ///
    /// # Errors
    /// See [`InstallError`]. The directory is left clean except for downloads worth resuming.
    pub async fn install(
        &self,
        scope: Scope,
        row: &'static Row,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<InstallDir, InstallError> {
        let dir = self.roots.dir(scope, row);
        if is_installed(&dir, row) {
            return Ok(dir);
        }
        let base = self.roots.engine_dir(scope, row.engine);
        make_dirs(&base)?;
        let need = row.download_size().saturating_mul(3);
        require_space(&base, need).map_err(|e| InstallError::Space {
            need: e.need,
            free: e.free,
        })?;
        let partial = dir.partial();
        let result = match row.delivery {
            Delivery::Archives(_) => self.archives(&dir, row, meter, cancel).await,
            Delivery::Python(_) => pyenv::build(&self.fetcher, &dir, row, meter, cancel).await,
        };
        match &result {
            Ok(()) => {
                let _clean = std::fs::remove_dir_all(&partial);
            }
            Err(error) if error.keeps_downloads() => {
                let _tree = std::fs::remove_dir_all(partial.join("engine"));
                let _cache = std::fs::remove_dir_all(partial.join("cache"));
                let _unfinished = std::fs::remove_dir_all(dir.path());
            }
            Err(_) => {
                let _clean = std::fs::remove_dir_all(&partial);
                let _unfinished = std::fs::remove_dir_all(dir.path());
            }
        }
        result.map(|()| dir)
    }

    /// Fetches every file of `row` into `<partial>/download`; returns their paths in order.
    pub(crate) async fn download(
        fetcher: &Fetcher,
        dir: &InstallDir,
        row: &Row,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<Vec<std::path::PathBuf>, InstallError> {
        let downloads = dir.partial().join("download");
        std::fs::create_dir_all(&downloads).map_err(|e| disk(&e))?;
        let mut paths = Vec::new();
        for artifact in row.artifacts() {
            let path = downloads.join(format!("{}.tar.gz", artifact.sha256));
            fetcher.fetch(&artifact, &path, meter, cancel).await?;
            paths.push(path);
        }
        Ok(paths)
    }

    async fn archives(
        &self,
        dir: &InstallDir,
        row: &'static Row,
        meter: &Meter,
        cancel: &CancelToken,
    ) -> Result<(), InstallError> {
        let files = Self::download(&self.fetcher, dir, row, meter, cancel).await?;
        let partial = dir.partial();
        let tree = partial.join("engine");
        let pinned = row.tree_seal;
        let (tree_here, record_at) = (tree.clone(), partial.join("installed.json"));
        let record = record_of(row);
        let cancelled = cancel.clone();
        blocking(move || {
            let _old = std::fs::remove_dir_all(&tree_here);
            std::fs::create_dir_all(&tree_here).map_err(|e| disk(&e))?;
            for file in &files {
                if cancelled.is_cancelled() {
                    return Err(InstallError::Cancelled);
                }
                unpack(file, &tree_here)?;
            }
            settle_modes(&tree_here)?;
            let seal = seal_of(&tree_here)?;
            if !pinned.is_some_and(|hex| seal.is(hex)) {
                return Err(InstallError::SealMismatch);
            }
            let record = Record {
                seal: seal.as_hex().to_owned(),
                ..record
            };
            record
                .write(&record_at)
                .map_err(|_| InstallError::Disk(std::io::ErrorKind::Other))
        })
        .await?;
        publish(&partial, dir)
    }
}

/// A record with what the row says and nothing sealed yet.
pub(crate) fn record_of(row: &Row) -> Record {
    Record {
        engine: row.engine.slug().to_owned(),
        version: row.version.to_owned(),
        backend: row.backend.slug().to_owned(),
        seal: String::new(),
        entries: std::collections::BTreeMap::new(),
        venv_seal: None,
    }
}

/// Moves a finished `partial` directory to where the install goes, dropping its downloads.
fn publish(partial: &Path, dir: &InstallDir) -> Result<(), InstallError> {
    let _downloads = std::fs::remove_dir_all(partial.join("download"));
    let _unfinished = std::fs::remove_dir_all(dir.path());
    std::fs::rename(partial, dir.path()).map_err(|e| disk(&e))
}

/// Whether `dir` holds a finished install of `row`.
fn is_installed(dir: &InstallDir, row: &Row) -> bool {
    Record::read(&dir.record()).is_ok_and(|record| {
        record.engine == row.engine.slug()
            && record.version == row.version
            && record.backend == row.backend.slug()
    }) && dir.program(row).exists()
}

/// Makes `path` and its missing parents, each world-readable: the root's directories must be
/// 0755 whatever the helper's umask is.
pub(crate) fn make_dirs(path: &Path) -> Result<(), InstallError> {
    let missing: Vec<_> = path
        .ancestors()
        .take_while(|p| !p.exists())
        .map(Path::to_path_buf)
        .collect();
    std::fs::create_dir_all(path).map_err(|e| disk(&e))?;
    for made in missing {
        std::fs::set_permissions(&made, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| disk(&e))?;
    }
    Ok(())
}

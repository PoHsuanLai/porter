//! The installs a program offers and runs: what could be installed for an engine here, the jobs
//! it starts and stops, and where each stands.
//!
//! An install for the person runs in this process, through [`Installer`]. An install for everyone
//! is the privileged helper started through `pkexec` ([`install_for_everyone`]): only an engine
//! and a backend are named to it. Either way the manifest of `porter-engines` decides what is
//! fetched. A job runs on the tokio runtime that called [`EngineInstalls::start`], and the person
//! who started it follows it with [`EngineInstalls::standing`] (counted in a [`Meter`]) and
//! stops it with [`EngineInstalls::cancel`].

use crate::elevated::{HelperProgram, install_for_everyone};
use crate::fetch::Fetcher;
use crate::installer::Installer;
use crate::progress::{CancelToken, Meter};
use porter_engines::{
    Backend, EngineId, Failure, Finder, Found, Host, Manifest, Roots, Row, Scope, pointed_strata,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

/// Why an install was not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// One is under way already.
    #[error("an install of this engine is already under way")]
    Busy,
    /// Nothing can be installed for this engine, here, in this way.
    #[error("nothing can be installed for this engine in this way on this computer")]
    Nothing,
}

/// What could be installed for an engine that is not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallChoice {
    /// The ways, everyone's first.
    pub scopes: Vec<Scope>,
    /// How many bytes a download is, when a pinned build is the way (a distro package's size is
    /// not known).
    pub size: Option<u64>,
}

/// What is known of one engine now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// It is here.
    Ready(Found),
    /// An install is under way, this many thousandths done.
    Installing(u32),
    /// The last install failed.
    Failed(Failure),
    /// It is not here.
    NotInstalled,
}

#[derive(Debug, Clone)]
struct Running {
    meter: Meter,
    cancel: CancelToken,
}

#[derive(Debug, Clone)]
enum Job {
    Running(Running),
    Failed(Failure),
}

/// Everything the installs are made of.
pub struct Parts {
    /// Where installs live.
    pub roots: Roots,
    /// What can be installed.
    pub manifest: Manifest,
    /// What this computer is.
    pub host: Host,
    /// The downloader.
    pub fetcher: Fetcher,
    /// The privileged helper and the program that starts it.
    pub helper: HelperProgram,
    /// The folder of a Strata the person pointed at, read each time it is looked for.
    pub strata_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    /// Whether a system path is one only root could have written (see `porter_daemon::root_held`).
    pub held: fn(&Path) -> bool,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("roots", &self.roots)
            .field("host", &self.host)
            .field("helper", &self.helper)
            .finish_non_exhaustive()
    }
}

struct Inner {
    manifest: Manifest,
    host: Host,
    finder: Finder,
    installer: Installer,
    helper: HelperProgram,
    strata_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    jobs: Mutex<BTreeMap<EngineId, Job>>,
    news: Arc<Notify>,
}

/// The installs of one program; clones share them.
#[derive(Clone)]
pub struct EngineInstalls {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for EngineInstalls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineInstalls").finish_non_exhaustive()
    }
}

impl EngineInstalls {
    /// Installs made of `parts`.
    pub fn new(parts: Parts) -> Self {
        let finder = Finder::new(parts.roots.clone(), parts.held).with_manifest(parts.manifest);
        Self {
            inner: Arc::new(Inner {
                manifest: parts.manifest,
                host: parts.host,
                finder,
                installer: Installer::new(parts.fetcher, parts.roots),
                helper: parts.helper,
                strata_folder: parts.strata_folder,
                jobs: Mutex::default(),
                news: Arc::new(Notify::new()),
            }),
        }
    }

    /// Woken each time an install starts, ends or fails.
    pub fn news(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.news)
    }

    fn jobs(&self) -> MutexGuard<'_, BTreeMap<EngineId, Job>> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The engine's program, if it is here now: a package's, an install of ours, or (Strata) the
    /// folder the person pointed at.
    pub fn find(&self, engine: EngineId) -> Option<Found> {
        let pointed = (engine == EngineId::Strata)
            .then(|| (self.inner.strata_folder)())
            .flatten()
            .and_then(|folder| pointed_strata(&folder));
        pointed.or_else(|| self.inner.finder.find(engine))
    }

    /// Where `engine` stands now.
    pub fn standing(&self, engine: EngineId) -> Standing {
        let job = self.jobs().get(&engine).cloned();
        match (job, self.find(engine)) {
            (Some(Job::Running(running)), _) => Standing::Installing(running.meter.permille()),
            (_, Some(found)) => Standing::Ready(found),
            (Some(Job::Failed(why)), None) => Standing::Failed(why),
            (None, None) => Standing::NotInstalled,
        }
    }

    /// What could be installed for `engine` on this computer, or `None` when nothing can: the
    /// ways (for everyone needs the helper and either a package or a pinned build; for the
    /// person needs a pinned build) and the size of the download when a build is the way.
    pub fn choice(&self, engine: EngineId) -> Option<InstallChoice> {
        let offer = self.inner.manifest.offer(engine, &self.inner.host);
        let package = offer
            .package
            .is_some_and(|source| Path::new(source.manager.program()).exists());
        let everyone = self.inner.helper.present() && (offer.row.is_some() || package);
        let scopes: Vec<Scope> = [
            everyone.then_some(Scope::AllUsers),
            offer.row.map(|_| Scope::JustMe),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!scopes.is_empty()).then(|| InstallChoice {
            scopes,
            size: offer.row.map(Row::download_size),
        })
    }

    /// Starts installing `engine` for `scope`, and answers at once.
    ///
    /// # Errors
    /// See [`StartError`].
    pub fn start(&self, engine: EngineId, scope: Scope) -> Result<(), StartError> {
        let offer = self.inner.manifest.offer(engine, &self.inner.host);
        let ways = self.choice(engine).map(|c| c.scopes).unwrap_or_default();
        if !ways.contains(&scope) {
            return Err(StartError::Nothing);
        }
        let total = match (scope, offer.row) {
            (Scope::JustMe, Some(row)) => row.download_size(),
            _ => 1000,
        };
        let running = Running {
            meter: Meter::new(total),
            cancel: CancelToken::new(),
        };
        {
            let mut jobs = self.jobs();
            if matches!(jobs.get(&engine), Some(Job::Running(_))) {
                return Err(StartError::Busy);
            }
            jobs.insert(engine, Job::Running(running.clone()));
        }
        self.inner.news.notify_waiters();
        let this = self.clone();
        match (scope, offer.row) {
            (Scope::JustMe, Some(row)) => {
                tokio::spawn(async move { this.person(engine, row, running).await });
            }
            _ => {
                let backend = offer.row.map_or(Backend::Cpu, |row| row.backend);
                tokio::spawn(async move { this.everyone(engine, backend, running).await });
            }
        }
        Ok(())
    }

    /// Stops the install of `engine`, if one is under way.
    pub fn cancel(&self, engine: EngineId) {
        if let Some(Job::Running(running)) = self.jobs().get(&engine) {
            running.cancel.cancel();
        }
    }

    /// Forgets the failure of the last install of `engine`.
    pub fn forget(&self, engine: EngineId) {
        let mut jobs = self.jobs();
        if matches!(jobs.get(&engine), Some(Job::Failed(_))) {
            jobs.remove(&engine);
        }
    }

    async fn person(self, engine: EngineId, row: &'static Row, running: Running) {
        // An install is a fresh one: what is in the directory (an install that was changed since
        // it was made, say) goes first.
        let dir = self.inner.installer.roots().dir(Scope::JustMe, row);
        let _stale = std::fs::remove_dir_all(dir.path());
        let result = self
            .inner
            .installer
            .install(Scope::JustMe, row, &running.meter, &running.cancel)
            .await;
        self.finish(engine, result.map(|_| ()).map_err(|e| e.failure()));
    }

    async fn everyone(self, engine: EngineId, backend: Backend, running: Running) {
        let result = install_for_everyone(
            &self.inner.helper,
            engine,
            backend,
            &running.meter,
            &running.cancel,
        )
        .await;
        self.finish(engine, result);
    }

    /// Records how a job ended: success and a stop leave nothing to say, a failure is kept until
    /// the next try.
    fn finish(&self, engine: EngineId, result: Result<(), Failure>) {
        {
            let mut jobs = self.jobs();
            match result {
                Ok(()) | Err(Failure::Cancelled) => {
                    jobs.remove(&engine);
                }
                Err(why) => {
                    jobs.insert(engine, Job::Failed(why));
                }
            }
        }
        self.inner.news.notify_waiters();
    }
}

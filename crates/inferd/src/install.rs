//! The engines Settings can install, as inferd sees them: what each engine's state is now (here,
//! being installed, failed, or not here and what could be installed), and the install jobs
//! Settings starts and stops.
//!
//! An install for the person runs here, in process, through `porter-engines-install`. An install
//! for everyone is the privileged helper started through `pkexec` (see [`system`]); inferd only
//! names an engine and a backend to it, and reads what it reports. Either way the manifest of
//! `porter-engines` decides what is fetched.

mod system;
#[cfg(test)]
mod tests;

use crate::hosts::LaunchGuard;
use crate::startup::Cause;
use porter_daemon::root_held;
use porter_dbus::InstallOffer;
use porter_engines::{
    Backend, EngineId, Failure, Finder, Found, Host, Manifest, Roots, Row, Scope, Verdict, Verifier,
};
use porter_engines_install::{CancelToken, Fetcher, Installer, Meter};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

pub use system::HelperProgram;

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

/// What is known of one engine now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// It is here.
    Ready(Found),
    /// An install is under way, this many thousandths done.
    Installing(u32),
    /// The last install failed.
    Failed(Failure),
    /// The person's own install of it is no longer the pinned one: it was changed since it was
    /// installed, and is not started until it is installed again.
    Changed,
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
}

/// The engines whose person-owned install was found changed.
type Tripped = Arc<Mutex<BTreeSet<EngineId>>>;

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
    roots: Roots,
    manifest: Manifest,
    host: Host,
    finder: Finder,
    installer: Installer,
    helper: HelperProgram,
    strata_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    jobs: Mutex<BTreeMap<EngineId, Job>>,
    tripped: Tripped,
    news: Arc<Notify>,
}

/// The installs of this daemon; clones share them.
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
        let finder = Finder::new(parts.roots.clone(), root_held).with_manifest(parts.manifest);
        Self {
            inner: Arc::new(Inner {
                roots: parts.roots.clone(),
                manifest: parts.manifest,
                host: parts.host,
                finder,
                installer: Installer::new(parts.fetcher, parts.roots.clone()),
                helper: parts.helper,
                strata_folder: parts.strata_folder,
                jobs: Mutex::default(),
                tripped: Tripped::default(),
                news: Arc::new(Notify::new()),
            }),
        }
    }

    /// Woken each time an install starts, ends or fails: whoever listens says `EnginesChanged`.
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
            .and_then(|folder| porter_engines::pointed_strata(&folder));
        pointed.or_else(|| self.inner.finder.find(engine))
    }

    /// The check that goes before every start of an engine program: a person-owned install must
    /// still be the pinned tree. Hand it to the host that starts the engines.
    pub fn guard(&self) -> Arc<dyn LaunchGuard> {
        Arc::new(PinnedPrograms {
            verifier: Verifier::new(self.inner.roots.clone(), self.inner.manifest),
            tripped: Arc::clone(&self.inner.tripped),
            news: Arc::clone(&self.inner.news),
        })
    }

    fn tripped(&self) -> MutexGuard<'_, BTreeSet<EngineId>> {
        self.inner
            .tripped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where `engine` stands now.
    pub fn standing(&self, engine: EngineId) -> Standing {
        let job = self.jobs().get(&engine).cloned();
        match job {
            Some(Job::Running(running)) => Standing::Installing(running.meter.permille()),
            other => match (self.find(engine), other) {
                (Some(_), _) if self.tripped().contains(&engine) => Standing::Changed,
                (Some(found), _) => Standing::Ready(found),
                (None, Some(Job::Failed(why))) => Standing::Failed(why),
                (None, _) => Standing::NotInstalled,
            },
        }
    }

    /// What could be installed for `engine` on this computer, or `None` when nothing can: the
    /// ways (for everyone needs the helper and either a package or a pinned build; for the
    /// person needs a pinned build) and the size of the download when a build is the way.
    pub fn offer(&self, engine: EngineId) -> Option<InstallOffer> {
        let offer = self.inner.manifest.offer(engine, &self.inner.host);
        let package = offer
            .package
            .is_some_and(|source| Path::new(source.manager.program()).exists());
        let everyone = self.inner.helper.present() && (offer.row.is_some() || package);
        let scopes: Vec<String> = [
            everyone.then_some(Scope::AllUsers),
            offer.row.map(|_| Scope::JustMe),
        ]
        .into_iter()
        .flatten()
        .map(|scope| scope.slug().to_owned())
        .collect();
        (!scopes.is_empty()).then(|| InstallOffer {
            size: offer.row.map(Row::download_size),
            scopes,
        })
    }

    /// Starts installing `engine` for `scope`, and answers at once.
    ///
    /// # Errors
    /// See [`StartError`].
    pub fn start(&self, engine: EngineId, scope: Scope) -> Result<(), StartError> {
        let offer = self.inner.manifest.offer(engine, &self.inner.host);
        let running = {
            let mut jobs = self.jobs();
            if matches!(jobs.get(&engine), Some(Job::Running(_))) {
                return Err(StartError::Busy);
            }
            let total = match (scope, offer.row) {
                (Scope::JustMe, Some(row)) => row.download_size(),
                _ => 1000,
            };
            let running = Running {
                meter: Meter::new(total),
                cancel: CancelToken::new(),
            };
            let ways = self.offer(engine).map(|o| o.scopes).unwrap_or_default();
            if !ways.iter().any(|way| way == scope.slug()) {
                return Err(StartError::Nothing);
            }
            jobs.insert(engine, Job::Running(running.clone()));
            running
        };
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

    async fn person(self, engine: EngineId, row: &'static Row, running: Running) {
        if self.tripped().contains(&engine) {
            // A changed install is replaced, not kept: whatever is in the directory goes.
            let _gone = std::fs::remove_dir_all(self.inner.roots.dir(Scope::JustMe, row).path());
        }
        let result = self
            .inner
            .installer
            .install(Scope::JustMe, row, &running.meter, &running.cancel)
            .await;
        self.finish(engine, result.map(|_| ()).map_err(|e| e.failure()));
    }

    async fn everyone(self, engine: EngineId, backend: Backend, running: Running) {
        let result = system::install(
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
        if result.is_ok() {
            self.tripped().remove(&engine);
        }
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

/// Checks a person's own install against the manifest's pins before it is started, and the whole
/// of a Python environment afterwards, in the background. What it finds changed it remembers: the
/// engine then stands as changed until it is installed again.
struct PinnedPrograms {
    verifier: Verifier,
    tripped: Tripped,
    news: Arc<Notify>,
}

impl PinnedPrograms {
    fn trip(tripped: &Tripped, news: &Notify, engine: EngineId) {
        tripped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(engine);
        news.notify_waiters();
    }
}

impl LaunchGuard for PinnedPrograms {
    fn check(&self, program: &Path) -> Result<(), Cause> {
        match self.verifier.check(program) {
            Verdict::NotChecked => Ok(()),
            Verdict::Changed(engine) => {
                Self::trip(&self.tripped, &self.news, engine);
                Err(Cause::ProgramChanged {
                    program: program.to_path_buf(),
                })
            }
            Verdict::Verified => {
                let (verifier, program) = (self.verifier.clone(), program.to_path_buf());
                let (tripped, news) = (Arc::clone(&self.tripped), Arc::clone(&self.news));
                std::thread::spawn(move || {
                    if let Verdict::Changed(engine) = verifier.full(&program) {
                        Self::trip(&tripped, &news, engine);
                    }
                });
                Ok(())
            }
        }
    }
}

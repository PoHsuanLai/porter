//! The engines of ours that are installed, as inferd meets them: where each is, which release,
//! and whether a person's own install is still the pinned one.
//!
//! inferd does not install anything: its unit has no network and no write access to the person's
//! files, and it holds their AI keys. Settings installs (`porter-engines-install`); inferd finds
//! what was installed and, before each start of a program of a person's own install, checks it
//! against the manifest's pins ([`Installed::guard`]). What a check finds changed it remembers: the
//! engine then stands as [`Standing::Changed`] and is not started until it is installed again.

use crate::hosts::LaunchGuard;
use crate::startup::Cause;
use porter_engines::{EngineId, Finder, Found, Manifest, Roots, Verdict, Verifier, pointed_strata};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The engines whose person-owned install was found changed.
type Tripped = Arc<Mutex<BTreeSet<EngineId>>>;

/// Where one engine stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// It is here, and it is as it was installed.
    Ready(Found),
    /// It is here, and a person's own install of it is no longer the pinned one.
    Changed(Found),
    /// It is not here.
    NotInstalled,
}

/// The installed engines of one daemon; clones share what they have found changed.
#[derive(Clone)]
pub struct Installed {
    roots: Roots,
    manifest: Manifest,
    finder: Finder,
    strata_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    tripped: Tripped,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Installed")
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl Installed {
    /// The engines installed under `roots` and pinned by `manifest`; `held` says whether a system
    /// path is one only root could have written, and `strata_folder` where a Strata the person
    /// pointed at is, read each time it is looked for.
    pub fn new(
        roots: Roots,
        manifest: Manifest,
        held: fn(&Path) -> bool,
        strata_folder: Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    ) -> Self {
        let finder = Finder::new(roots.clone(), held).with_manifest(manifest);
        Self {
            roots,
            manifest,
            finder,
            strata_folder,
            tripped: Tripped::default(),
        }
    }

    fn tripped(&self) -> MutexGuard<'_, BTreeSet<EngineId>> {
        self.tripped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where `engine` stands now: the folder of a Strata the person pointed at counts first.
    pub fn standing(&self, engine: EngineId) -> Standing {
        let pointed = (engine == EngineId::Strata)
            .then(|| (self.strata_folder)())
            .flatten()
            .and_then(|folder| pointed_strata(&folder));
        match pointed.or_else(|| self.finder.find(engine)) {
            Some(found) if self.tripped().contains(&engine) => Standing::Changed(found),
            Some(found) => Standing::Ready(found),
            None => Standing::NotInstalled,
        }
    }

    /// The check that goes before every start of an engine program: a person-owned install must
    /// still be the pinned tree. Hand it to the host that starts the engines.
    pub fn guard(&self) -> Arc<dyn LaunchGuard> {
        Arc::new(PinnedPrograms {
            verifier: Verifier::new(self.roots.clone(), self.manifest),
            tripped: Arc::clone(&self.tripped),
        })
    }
}

/// Checks a person's own install against the manifest's pins before it is started, and the whole
/// of a Python environment afterwards, in the background.
struct PinnedPrograms {
    verifier: Verifier,
    tripped: Tripped,
}

fn trip(tripped: &Tripped, engine: EngineId) {
    tripped
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(engine);
}

impl LaunchGuard for PinnedPrograms {
    fn check(&self, program: &Path) -> Result<(), Cause> {
        match self.verifier.check(program) {
            Verdict::NotChecked => Ok(()),
            Verdict::Changed(engine) => {
                trip(&self.tripped, engine);
                Err(Cause::ProgramChanged {
                    program: program.to_path_buf(),
                })
            }
            Verdict::Verified => {
                let (verifier, program) = (self.verifier.clone(), program.to_path_buf());
                let tripped = Arc::clone(&self.tripped);
                std::thread::spawn(move || {
                    if let Verdict::Changed(engine) = verifier.full(&program) {
                        trip(&tripped, engine);
                    }
                });
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests;

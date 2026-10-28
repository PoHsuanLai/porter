//! The program each engine is started with, which the person may change while inferd runs
//! (`engines.vllm_python`, `engines.llama_server`). The units of the engines are made once, at
//! start, with the programs found then. [`Programs`] is told what the file names now, and the
//! driver puts the program now in force into the unit of an engine as it is started: an engine
//! that is running keeps running, the next start uses the new program.
//!
//! The program is the only thing of a unit this touches; the host still checks it as it starts
//! the engine (`hosts::LaunchGuard`), whichever program it is.

use engine_spec::ProgramPath;
use engine_supervisor::UnitSpec;
use porter_router::local::EngineConfig;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The programs in force. Cloning shares them.
#[derive(Debug, Clone, Default)]
pub struct Programs {
    shared: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    vllm: Choice,
    llama: Choice,
}

/// What one program is, and how it came to be.
#[derive(Debug, Default, Clone)]
struct Choice {
    /// The program the units were made with at start: a unit that names it is the engine's.
    made_with: Option<PathBuf>,
    /// The program inferd finds when the file names none.
    found: Option<PathBuf>,
    /// The program the file names now.
    named: Option<PathBuf>,
}

impl Choice {
    /// The program to start with: the file's, else the one found.
    fn now(&self) -> Option<&PathBuf> {
        self.named.as_ref().or(self.found.as_ref())
    }

    fn apply(&self, unit: &mut UnitSpec) {
        if let (Some(made), Some(now)) = (&self.made_with, self.now())
            && unit.program.0 == *made
        {
            unit.program = ProgramPath(now.clone());
        }
    }
}

fn held(lock: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Programs {
    /// The programs `made` was made with (what the units name), and what inferd finds for each
    /// when the file names none (`vllm_python`, `llama_server`). Nothing is named yet: see
    /// [`Self::follow`].
    pub fn new(
        made: &EngineConfig,
        vllm_python: Option<PathBuf>,
        llama_server: Option<PathBuf>,
    ) -> Self {
        let choice = |made: &Option<PathBuf>, found: Option<PathBuf>| Choice {
            made_with: made.clone(),
            found,
            named: None,
        };
        Self {
            shared: Arc::new(Mutex::new(Inner {
                vllm: choice(&made.vllm_python, vllm_python),
                llama: choice(&made.llama_server, llama_server),
            })),
        }
    }

    /// Takes the programs `file` names (none for a program it does not name).
    pub fn follow(&self, file: &EngineConfig) {
        let mut inner = held(&self.shared);
        inner.vllm.named = file.vllm_python.clone();
        inner.llama.named = file.llama_server.clone();
    }

    /// `unit` with the program now in force in place of the one it was made with.
    pub fn apply(&self, mut unit: UnitSpec) -> UnitSpec {
        let inner = held(&self.shared);
        inner.vllm.apply(&mut unit);
        inner.llama.apply(&mut unit);
        unit
    }

    /// `base` with the programs now in force.
    pub fn over(&self, base: &EngineConfig) -> EngineConfig {
        let inner = held(&self.shared);
        EngineConfig {
            vllm_python: inner
                .vllm
                .now()
                .cloned()
                .or_else(|| base.vllm_python.clone()),
            llama_server: inner
                .llama
                .now()
                .cloned()
                .or_else(|| base.llama_server.clone()),
            ..base.clone()
        }
    }
}

//! Starting, stopping and listing the models inferd runs itself: what `Inference1.Start`, `Stop`
//! and `Running` ask of the engines. A model the person attached, or one that runs in a program
//! of theirs on a loopback port, is not inferd's to start or stop: both are refused in plain
//! words, and neither is listed.

use super::*;
use engine_supervisor::EngineFailure;
use model_catalog::{EngineKind, MiB};
use porter_dbus::{RunState, RunningModel};
use zbus::fdo;

/// Why a model could not be started or stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// No model of that name is known here.
    Unknown,
    /// The person attached the engine: it runs where they set it up.
    Attached,
    /// A program of the person's on this computer serves it.
    Runtime,
    /// The model's files are not on this computer yet.
    NotDownloaded,
    /// The engine did not come up, in plain words.
    Failed(String),
}

impl From<RunError> for fdo::Error {
    fn from(error: RunError) -> Self {
        let words = |text: &str| text.to_owned();
        match error {
            RunError::Unknown => {
                fdo::Error::InvalidArgs(words("This computer does not know that model."))
            }
            RunError::Attached => fdo::Error::NotSupported(words(
                "That model runs on an engine you set up yourself; start and stop it where it runs.",
            )),
            RunError::Runtime => fdo::Error::NotSupported(words(
                "That model runs in a program you started yourself; start and stop it there.",
            )),
            RunError::NotDownloaded => fdo::Error::Failed(words(
                "That model has not been downloaded to this computer yet.",
            )),
            RunError::Failed(why) => fdo::Error::Failed(why),
        }
    }
}

/// A memory size as a person reads it: gigabytes, to the nearest tenth.
fn gigabytes(mib: MiB) -> String {
    let tenths = (u64::from(mib.0) * 10 + 512) / 1024;
    format!("{}.{} GB", tenths / 10, tenths % 10)
}

/// Why an engine could not start, in plain words.
pub fn start_words(cause: &Cause) -> String {
    match cause {
        Cause::NoRoom { need, free } => format!(
            "needs {} of graphics memory; {} is free",
            gigabytes(*need),
            gigabytes(*free)
        ),
        Cause::Exited { code, .. } if code.0 < 0 => {
            "the engine was stopped while starting".to_owned()
        }
        Cause::Exited { code, .. } => {
            format!("the engine stopped while starting (exit {})", code.0)
        }
        Cause::NeverReady { .. } => "the engine did not become ready in time".to_owned(),
        Cause::SocketPathTooLong { .. }
        | Cause::SocketPathTaken { .. }
        | Cause::SocketPathUnusable { .. } => {
            "the engine could not be given a place to listen".to_owned()
        }
        Cause::CannotSpawn { .. } => "the engine's program could not be run".to_owned(),
        Cause::ProgramChanged { .. } => {
            "the engine's program was changed since it was installed, so it was not started"
                .to_owned()
        }
        Cause::Refused => "the engine could not be started".to_owned(),
        Cause::BadProfile => "this computer cannot run the model as it is set up".to_owned(),
        Cause::Attached(_) => "the engine runs where you set it up, not here".to_owned(),
        Cause::Unknown => "the engine is not known here".to_owned(),
        Cause::Released => "the model was stopped".to_owned(),
    }
}

/// The cause a failed state reads as when the supervisor kept none (a failure it has from before
/// its record).
fn cause_of(failure: EngineFailure) -> Cause {
    match failure {
        EngineFailure::NoRoom { need, free } => Cause::NoRoom { need, free },
        EngineFailure::Exited { code } => Cause::Exited {
            code,
            tail: crate::startup::Tail::default(),
        },
        EngineFailure::BadProfile => Cause::BadProfile,
        // never ready, or a failure a later supervisor adds
        _ => Cause::NeverReady {
            tail: crate::startup::Tail::default(),
        },
    }
}

/// The word for an engine kind on the bus.
fn engine_word(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Vllm => "vllm",
        EngineKind::LlamaServer => "llama-server",
        EngineKind::SpeechHost => "speech-host",
        EngineKind::KokoroFastApi => "kokoro",
    }
}

impl Engines {
    /// The model `text` (`account/model`) when inferd runs it itself.
    fn run_by_us(&self, text: &str) -> Result<Arc<LocalModel>, RunError> {
        let model = ModelRef::parse(text).ok_or(RunError::Unknown)?;
        let local = self.local(&model).ok_or(RunError::Unknown)?;
        if local.attached.is_some() {
            Err(RunError::Attached)
        } else if local.loopback.is_some() {
            Err(RunError::Runtime)
        } else {
            Ok(local)
        }
    }

    /// Starts the engine of the model `text` (`account/model`) and waits until it answers.
    ///
    /// # Errors
    /// See [`RunError`]. An engine that failed so lately that no request may start it again is
    /// answered with that failure at once.
    pub async fn start_model(&self, text: &str) -> Result<(), RunError> {
        let local = self.run_by_us(text)?;
        if local.weights() == Weights::Missing {
            return Err(RunError::NotDownloaded);
        }
        self.supervised
            .want(&local.spec.id)
            .await
            .map_err(|failed| RunError::Failed(start_words(&failed.cause)))
    }

    /// Stops the engine of the model `text` (`account/model`); nothing starts it again until it
    /// is wanted. A model already stopped is not an error.
    ///
    /// # Errors
    /// See [`RunError`].
    pub fn stop_model(&self, text: &str) -> Result<(), RunError> {
        let local = self.run_by_us(text)?;
        self.supervised.release(&local.spec.id);
        Ok(())
    }

    /// Every model inferd runs itself and what its engine is doing, in the order of the book.
    pub fn running(&self) -> Vec<RunningModel> {
        let snapshot = self.supervised.snapshot();
        self.book
            .local
            .iter()
            .filter(|model| model.attached.is_none() && model.loopback.is_none())
            .filter_map(|model| {
                let id = &model.spec.id;
                let state = *snapshot.states.get(id)?;
                let (run, began) = match state {
                    EngineState::Stopped => (RunState::Stopped, None),
                    EngineState::Starting { since, .. } => (RunState::Starting, Some(since)),
                    EngineState::Ready { since, .. } => (RunState::Ready, Some(since)),
                    EngineState::Stopping { since } => (RunState::Stopping, Some(since)),
                    EngineState::Backoff { .. } => (RunState::Retrying, None),
                    EngineState::Failed(_) => (RunState::Failed, None),
                    // a state a later supervisor adds: not listed until porter knows it
                    _ => return None,
                };
                let memory = self.supervised.memory_of(id, model.spec.need);
                let mut row = RunningModel::new(
                    model.model_ref().to_string(),
                    run,
                    engine_word(model.profile.kind),
                    memory.0,
                );
                if let Some(date) = began.and_then(|since| self.supervised.wall_clock(since)) {
                    row = row.since(date);
                }
                if run == RunState::Ready
                    && let Some(window) = self.supervised.window_of(id)
                {
                    row = row.window(window.0);
                }
                if let EngineState::Failed(failure) = state {
                    let cause = snapshot
                        .failures
                        .get(id)
                        .map_or_else(|| cause_of(failure), |kept| kept.cause.clone());
                    row = row.reason(start_words(&cause));
                }
                Some(row)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_supervisor::ExitCode;

    #[test]
    fn a_failure_is_told_in_plain_words() {
        let none = crate::startup::Tail::default;
        for (cause, words) in [
            (
                Cause::NoRoom {
                    need: MiB(14_540),
                    free: MiB(6_246),
                },
                "needs 14.2 GB of graphics memory; 6.1 GB is free",
            ),
            (
                Cause::Exited {
                    code: ExitCode(3),
                    tail: none(),
                },
                "the engine stopped while starting (exit 3)",
            ),
            (
                Cause::Exited {
                    code: ExitCode(-1),
                    tail: none(),
                },
                "the engine was stopped while starting",
            ),
            (
                Cause::NeverReady { tail: none() },
                "the engine did not become ready in time",
            ),
        ] {
            assert_eq!(start_words(&cause), words);
        }
    }
}

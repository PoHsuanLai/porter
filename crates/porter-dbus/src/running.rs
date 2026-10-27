//! The D-Bus shape of a model inferd runs itself (`(sa{sv})`, a row of `Inference1.Running`): the
//! model (`account/model`), then a vardict of what its engine is doing. A client reads the typed
//! [`RunningModel`], never the strings.

use crate::args::Details;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zbus::zvariant::{OwnedValue, Value};

/// The vardict key of the state (`s`): see [`RunState`].
pub const RUNNING_KEY_STATE: &str = "state";
/// The vardict key of the engine kind (`s`): `vllm`, `llama-server`, `speech-host` or `kokoro`.
pub const RUNNING_KEY_ENGINE: &str = "engine";
/// The vardict key of the graphics memory the engine takes, in MiB (`u`).
pub const RUNNING_KEY_MEMORY_MIB: &str = "memory_mib";
/// The vardict key of when the state began, in unix seconds (`x`); absent while stopped.
pub const RUNNING_KEY_SINCE: &str = "since";
/// The vardict key of the tokens the engine was started with (`u`); present only while ready,
/// and only for an engine whose window is chosen when it starts.
pub const RUNNING_KEY_WINDOW: &str = "window";
/// The vardict key of why the engine failed, in plain words (`s`); present only while failed.
pub const RUNNING_KEY_REASON: &str = "reason";

/// What an engine is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunState {
    /// Not running, and nothing wants it.
    Stopped,
    /// Starting, not yet answering.
    Starting,
    /// Running and answering.
    Ready,
    /// Being stopped.
    Stopping,
    /// Ended on its own and waiting to be tried again.
    Retrying,
    /// Could not be started.
    Failed,
}

impl RunState {
    /// The word on the wire.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
        }
    }

    /// The state a word names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        [
            Self::Stopped,
            Self::Starting,
            Self::Ready,
            Self::Stopping,
            Self::Retrying,
            Self::Failed,
        ]
        .into_iter()
        .find(|state| state.slug() == text)
    }
}

/// One model inferd runs itself, and what its engine is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunningModel {
    /// The model, `account/model`.
    pub model: String,
    /// What its engine is doing.
    pub state: RunState,
    /// The engine kind (`vllm`, `llama-server`, `speech-host`, `kokoro`).
    pub engine: String,
    /// The graphics memory it takes, in MiB.
    pub memory_mib: u32,
    /// When the state began; none while stopped.
    pub since: Option<SystemTime>,
    /// The tokens it was started with; only while ready.
    pub window: Option<u32>,
    /// Why it failed, in plain words; only while failed.
    pub reason: Option<String>,
}

impl RunningModel {
    /// A model in `state`, run by `engine` (a kind's word), taking `memory_mib`.
    pub fn new(
        model: impl Into<String>,
        state: RunState,
        engine: impl Into<String>,
        memory_mib: u32,
    ) -> Self {
        Self {
            model: model.into(),
            state,
            engine: engine.into(),
            memory_mib,
            since: None,
            window: None,
            reason: None,
        }
    }

    /// The same, whose state began at `since`.
    pub fn since(self, since: SystemTime) -> Self {
        Self {
            since: Some(since),
            ..self
        }
    }

    /// The same, started with `window` tokens.
    pub fn window(self, window: u32) -> Self {
        Self {
            window: Some(window),
            ..self
        }
    }

    /// The same, failed for `reason`.
    pub fn reason(self, reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..self
        }
    }
}

/// Why a row of `Running` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RunningRowError {
    /// A key the interface promises is absent or of another type.
    #[error("no {0}")]
    Missing(&'static str),
    /// The state is not one this version knows.
    #[error("unknown state {0:?}")]
    State(String),
}

fn owned(value: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(value).ok()
}

fn unix_seconds(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

/// A model as its id and a vardict of the rest.
pub fn running_to_dbus(running: &RunningModel) -> (String, Details) {
    let fields = [
        (RUNNING_KEY_STATE, owned(Value::from(running.state.slug()))),
        (
            RUNNING_KEY_ENGINE,
            owned(Value::from(running.engine.clone())),
        ),
        (
            RUNNING_KEY_MEMORY_MIB,
            owned(Value::U32(running.memory_mib)),
        ),
        (
            RUNNING_KEY_SINCE,
            running
                .since
                .and_then(|at| owned(Value::I64(unix_seconds(at)))),
        ),
        (
            RUNNING_KEY_WINDOW,
            running.window.and_then(|tokens| owned(Value::U32(tokens))),
        ),
        (
            RUNNING_KEY_REASON,
            running
                .reason
                .clone()
                .and_then(|words| owned(Value::from(words))),
        ),
    ];
    let details = fields
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (running.model.clone(), details)
}

fn value(details: &Details, key: &str) -> Option<OwnedValue> {
    details.get(key).and_then(|value| value.try_clone().ok())
}

fn text(details: &Details, key: &'static str) -> Result<String, RunningRowError> {
    value(details, key)
        .and_then(|value| String::try_from(value).ok())
        .ok_or(RunningRowError::Missing(key))
}

/// A row of `Running` as a [`RunningModel`].
///
/// # Errors
/// See [`RunningRowError`].
pub fn running_from_dbus(
    (model, details): (String, Details),
) -> Result<RunningModel, RunningRowError> {
    let word = text(&details, RUNNING_KEY_STATE)?;
    let state = RunState::from_slug(&word).ok_or(RunningRowError::State(word))?;
    let memory_mib = value(&details, RUNNING_KEY_MEMORY_MIB)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(RunningRowError::Missing(RUNNING_KEY_MEMORY_MIB))?;
    let since = value(&details, RUNNING_KEY_SINCE)
        .and_then(|value| i64::try_from(value).ok())
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
    Ok(RunningModel {
        engine: text(&details, RUNNING_KEY_ENGINE)?,
        model,
        state,
        memory_mib,
        since,
        window: value(&details, RUNNING_KEY_WINDOW).and_then(|value| u32::try_from(value).ok()),
        reason: text(&details, RUNNING_KEY_REASON).ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(state: RunState) -> RunningModel {
        RunningModel::new("local/qwen3-8b", state, "vllm", 14_540)
    }

    #[test]
    fn a_row_round_trips_and_a_stopped_model_carries_only_what_it_has() {
        let at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ready = model(RunState::Ready).since(at).window(32_768);
        let failed = model(RunState::Failed)
            .since(at)
            .reason("the engine did not become ready in time");
        let stopped = model(RunState::Stopped);
        for running in [ready, failed, stopped] {
            let row = running_to_dbus(&running);
            assert_eq!(
                row.1.contains_key(RUNNING_KEY_SINCE),
                running.state != RunState::Stopped
            );
            assert_eq!(running_from_dbus(row), Ok(running));
        }
    }

    #[test]
    fn a_row_that_says_less_than_promised_or_an_unknown_state_is_refused() {
        let (id, mut details) = running_to_dbus(&model(RunState::Stopped));
        details.insert(
            RUNNING_KEY_STATE.into(),
            owned(Value::from("odd")).expect("value"),
        );
        assert_eq!(
            running_from_dbus((id.clone(), details)),
            Err(RunningRowError::State("odd".into()))
        );
        let (_, mut details) = running_to_dbus(&model(RunState::Stopped));
        details.remove(RUNNING_KEY_MEMORY_MIB);
        assert_eq!(
            running_from_dbus((id, details)),
            Err(RunningRowError::Missing(RUNNING_KEY_MEMORY_MIB))
        );
    }
}

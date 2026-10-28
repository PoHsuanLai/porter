//! What a failed request is told as, and what the log says about it. A failure ends the one
//! request or session it hit and says why in plain words; the daemon and every other session go
//! on. Pure: the engine's state and the memory numbers of the moment in, the error out.

use crate::admit::Priority;
use engine_supervisor::EngineId;
use porter_ids::Bytes;
use porter_infer::{HardwareFacts, MemoryFacts, ModelError};

/// Whether the engine behind a request is serving now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineView {
    /// It is ready.
    Running,
    /// It is stopped, starting again, backing off or failed.
    NotRunning,
}

/// What the machine looked like when a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// The engine the request was on.
    pub engine: EngineView,
    /// Free graphics memory against what one more request needs, when known.
    pub memory: Option<MemoryFacts>,
}

/// What a request that ended with `error` is told. A request the engine could not be reached for
/// is told as the engine having stopped when it is not running now, and as running out of
/// graphics memory when it is running and the card has less free than one more request needs.
/// Any other error stands as it is: a refused prompt stays refused, a timeout stays a timeout.
pub fn explain(error: ModelError, moment: &Moment) -> ModelError {
    match (error, moment.engine, moment.memory) {
        (ModelError::Unreachable, EngineView::NotRunning, _) => ModelError::EngineStopped,
        (ModelError::Unreachable, EngineView::Running, Some(memory))
            if memory.free < memory.one_more =>
        {
            ModelError::OutOfMemory
        }
        (error, _, _) => error,
    }
}

/// Whether the failed request had already sent the client anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spoke {
    /// Nothing yet: a second try would not repeat anything the client has.
    Nothing,
    /// Events went out: a second try would repeat them.
    Something,
}

/// Whether the request has been tried on another instance already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tried {
    /// This was the first try.
    Never,
    /// It already moved once.
    Once,
}

/// What to do with a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Run it again on another healthy instance of the same model.
    Elsewhere,
    /// It fails, with its plain sentence.
    No,
}

/// The person's own request that was on an engine that stopped, before it sent anything, is run
/// once more on another healthy instance of the same model (`others` of them). Nothing else is
/// retried, and never on a different model: there is no quiet fallback.
pub fn retry(who: Priority, error: ModelError, spoke: Spoke, tried: Tried, others: usize) -> Retry {
    match (who, error, spoke, tried, others) {
        (Priority::Person, ModelError::EngineStopped, Spoke::Nothing, Tried::Never, 1..) => {
            Retry::Elsewhere
        }
        _ => Retry::No,
    }
}

fn mebibytes(bytes: Bytes) -> u64 {
    bytes.0 >> 20
}

/// The one line the log gets for a failure, with the hardware numbers of the moment.
pub fn log_line(what: &str, engine: Option<&EngineId>, facts: &HardwareFacts) -> String {
    let memory = facts.memory.map_or_else(
        || "graphics memory not read".to_owned(),
        |memory| {
            format!(
                "graphics memory {} MiB free, about {} MiB for one more",
                mebibytes(memory.free),
                mebibytes(memory.one_more)
            )
        },
    );
    let engine = engine.map_or("no engine", |engine| engine.0.as_str());
    format!(
        "inferd: {what} on {engine}: {memory}; {} helpers using it, {} waiting",
        facts.helpers.0, facts.ahead.0
    )
}

#[cfg(test)]
mod tests;

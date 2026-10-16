//! The D-Bus shape of a local engine (`(sa{sv})`, a row of `Inference1.LocalEngines`): the
//! engine's id, then a vardict of its name, its state, its path when it is found and the slots it
//! would serve. A client reads the typed [`LocalEngine`], never the strings.

use crate::args::Details;
use std::path::PathBuf;
use zbus::zvariant::{OwnedValue, Value};

/// The vardict key of what people call the engine (`s`).
pub const ENGINE_KEY_NAME: &str = "name";
/// The vardict key of the state (`s`): `found` or `missing`.
pub const ENGINE_KEY_STATE: &str = "state";
/// The vardict key of the program's path (`s`): present only when the state is `found`.
pub const ENGINE_KEY_PATH: &str = "path";
/// The vardict key of the slots the engine would serve (`as`, the slot slugs).
pub const ENGINE_KEY_SLOTS: &str = "slots";

/// The state word of an engine that has a program on this computer.
const FOUND: &str = "found";
/// The state word of an engine with no program, or whose path is not there.
const MISSING: &str = "missing";

/// Whether an engine's program is on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// The program is there, at this path.
    Found(PathBuf),
    /// No path is set or discovered, or the path is not there.
    Missing,
}

/// One engine kind inferd knows, and whether it can run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEngine {
    /// The engine's stable id (`llama_cpp`, `vllm`, `speech`, `kokoro`).
    pub id: String,
    /// What people call it.
    pub name: String,
    /// Whether its program is here.
    pub state: EngineState,
    /// The slot slugs it would serve.
    pub slots: Vec<String>,
}

/// Why a row of `LocalEngines` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineRowError {
    /// A key the interface promises is absent or of another type.
    #[error("no {0}")]
    Missing(&'static str),
    /// The state is neither `found` nor `missing`.
    #[error("unknown state {0:?}")]
    State(String),
    /// The state is `found` and no path came with it.
    #[error("found without a path")]
    NoPath,
}

fn owned(value: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(value).ok()
}

/// An engine as its id and a vardict of the rest.
pub fn local_engine_to_dbus(engine: &LocalEngine) -> (String, Details) {
    let (state, path) = match &engine.state {
        EngineState::Found(path) => (FOUND, Some(path.to_string_lossy().into_owned())),
        EngineState::Missing => (MISSING, None),
    };
    let fields = [
        (ENGINE_KEY_NAME, owned(Value::from(engine.name.clone()))),
        (ENGINE_KEY_STATE, owned(Value::from(state))),
        (ENGINE_KEY_PATH, path.and_then(|p| owned(Value::from(p)))),
        (
            ENGINE_KEY_SLOTS,
            owned(Value::Array(engine.slots.clone().into())),
        ),
    ];
    let details = fields
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (engine.id.clone(), details)
}

fn text(details: &Details, key: &'static str) -> Result<String, EngineRowError> {
    details
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| String::try_from(value).ok())
        .ok_or(EngineRowError::Missing(key))
}

/// A row of `LocalEngines` as a [`LocalEngine`].
///
/// # Errors
/// See [`EngineRowError`].
pub fn local_engine_from_dbus(
    (id, details): (String, Details),
) -> Result<LocalEngine, EngineRowError> {
    let state = match text(&details, ENGINE_KEY_STATE)?.as_str() {
        FOUND => EngineState::Found(PathBuf::from(
            text(&details, ENGINE_KEY_PATH).map_err(|_| EngineRowError::NoPath)?,
        )),
        MISSING => EngineState::Missing,
        other => return Err(EngineRowError::State(other.to_owned())),
    };
    let slots = details
        .get(ENGINE_KEY_SLOTS)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .ok_or(EngineRowError::Missing(ENGINE_KEY_SLOTS))?;
    Ok(LocalEngine {
        name: text(&details, ENGINE_KEY_NAME)?,
        id,
        state,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(state: EngineState) -> LocalEngine {
        LocalEngine {
            id: "llama_cpp".into(),
            name: "llama.cpp".into(),
            state,
            slots: vec!["text".into(), "image_in".into()],
        }
    }

    #[test]
    fn a_row_round_trips_and_a_missing_engine_carries_no_path() {
        let found = engine(EngineState::Found(PathBuf::from("/usr/bin/llama-server")));
        let missing = engine(EngineState::Missing);
        for engine in [found, missing] {
            let row = local_engine_to_dbus(&engine);
            assert_eq!(
                row.1.contains_key(ENGINE_KEY_PATH),
                engine.state != EngineState::Missing
            );
            assert_eq!(local_engine_from_dbus(row), Ok(engine));
        }
    }

    #[test]
    fn a_row_that_says_less_than_promised_is_refused() {
        let (id, mut details) = local_engine_to_dbus(&engine(EngineState::Found("/x".into())));
        details.remove(ENGINE_KEY_PATH);
        assert_eq!(
            local_engine_from_dbus((id.clone(), details)),
            Err(EngineRowError::NoPath)
        );
        let (_, mut details) = local_engine_to_dbus(&engine(EngineState::Missing));
        details.insert(
            ENGINE_KEY_STATE.into(),
            owned(Value::from("odd")).expect("value"),
        );
        assert_eq!(
            local_engine_from_dbus((id, details)),
            Err(EngineRowError::State("odd".into()))
        );
    }
}

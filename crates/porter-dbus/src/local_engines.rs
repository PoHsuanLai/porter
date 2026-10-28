//! The D-Bus shape of a local engine (`(sa{sv})`, a row of `Inference1.LocalEngines`): the
//! engine's id, then a vardict of its name, its state, its path when it is found, the slots it
//! would serve and, where they apply, its release and where it came from. A client reads the typed
//! [`LocalEngine`], never the strings.

use crate::args::Details;
use porter_ids::word::{from_word, word};
use serde::{Deserialize, Serialize};
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
/// The vardict key of the engine's release (`s`), when it is known.
pub const ENGINE_KEY_VERSION: &str = "version";
/// The vardict key of where a found engine came from (`s`): `package`, `system`, `person` or
/// `pointed`.
pub const ENGINE_KEY_PLACE: &str = "place";

/// The state word of an engine that has a program on this computer.
const FOUND: &str = "found";
/// The state word of an engine with no program, or whose path is not there.
const MISSING: &str = "missing";
/// The state word of an install of ours that is no longer the pinned one.
const CHANGED: &str = "changed";

/// Whether an engine's program is on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// The program is there, at this path.
    Found(PathBuf),
    /// No path is set or discovered, or the path is not there.
    Missing,
    /// The program is there and is an install of ours that is no longer the pinned one: it was
    /// changed since it was installed, and is not started until it is installed again.
    Changed(PathBuf),
}

/// Where a found engine came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePlace {
    /// The distro's own package.
    Package,
    /// An install for everyone, made by us.
    System,
    /// An install for this person, made by us.
    Person,
    /// A folder the person pointed at.
    Pointed,
}

impl EnginePlace {
    /// The word on the wire.
    pub fn slug(self) -> &'static str {
        word(&self)
    }

    /// The place a word names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        from_word(text)
    }
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
    /// Its release, when known.
    pub version: Option<String>,
    /// Where it came from, when it is found.
    pub place: Option<EnginePlace>,
}

/// Why a row of `LocalEngines` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    /// The place is not one this version knows.
    #[error("unknown place {0:?}")]
    Place(String),
}

fn owned(value: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(value).ok()
}

/// An engine as its id and a vardict of the rest.
pub fn local_engine_to_dbus(engine: &LocalEngine) -> (String, Details) {
    let (state, path) = match &engine.state {
        EngineState::Found(path) => (FOUND, Some(path.to_string_lossy().into_owned())),
        EngineState::Changed(path) => (CHANGED, Some(path.to_string_lossy().into_owned())),
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
        (
            ENGINE_KEY_VERSION,
            engine.version.clone().and_then(|v| owned(Value::from(v))),
        ),
        (
            ENGINE_KEY_PLACE,
            engine.place.and_then(|p| owned(Value::from(p.slug()))),
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
    let path = || text(&details, ENGINE_KEY_PATH).map_err(|_| EngineRowError::NoPath);
    let state = match text(&details, ENGINE_KEY_STATE)?.as_str() {
        FOUND => EngineState::Found(PathBuf::from(path()?)),
        CHANGED => EngineState::Changed(PathBuf::from(path()?)),
        MISSING => EngineState::Missing,
        other => return Err(EngineRowError::State(other.to_owned())),
    };
    let slots = details
        .get(ENGINE_KEY_SLOTS)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .ok_or(EngineRowError::Missing(ENGINE_KEY_SLOTS))?;
    let place = match text(&details, ENGINE_KEY_PLACE) {
        Ok(word) => Some(EnginePlace::from_slug(&word).ok_or(EngineRowError::Place(word))?),
        Err(_) => None,
    };
    Ok(LocalEngine {
        name: text(&details, ENGINE_KEY_NAME)?,
        id,
        state,
        slots,
        version: text(&details, ENGINE_KEY_VERSION).ok(),
        place,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_place_keeps_the_word_it_has_always_had_on_the_bus() {
        let words = [
            (EnginePlace::Package, "package"),
            (EnginePlace::System, "system"),
            (EnginePlace::Person, "person"),
            (EnginePlace::Pointed, "pointed"),
        ];
        for (place, word) in words {
            assert_eq!(place.slug(), word);
            assert_eq!(EnginePlace::from_slug(word), Some(place));
        }
        assert_eq!(EnginePlace::from_slug("Package"), None);
    }

    fn engine(state: EngineState) -> LocalEngine {
        LocalEngine {
            id: "llama_cpp".into(),
            name: "llama.cpp".into(),
            state,
            slots: vec!["text".into(), "image_in".into()],
            version: None,
            place: None,
        }
    }

    #[test]
    fn a_row_round_trips_and_a_missing_engine_carries_no_path() {
        let found = LocalEngine {
            version: Some("b11669".into()),
            place: Some(EnginePlace::Person),
            ..engine(EngineState::Found(PathBuf::from("/usr/bin/llama-server")))
        };
        let changed = engine(EngineState::Changed(PathBuf::from(
            "/home/p/engine/llama-server",
        )));
        let missing = engine(EngineState::Missing);
        for engine in [found, changed, missing] {
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
            local_engine_from_dbus((id.clone(), details)),
            Err(EngineRowError::State("odd".into()))
        );
        let (_, mut details) = local_engine_to_dbus(&engine(EngineState::Missing));
        details.insert(
            ENGINE_KEY_PLACE.into(),
            owned(Value::from("moon")).expect("value"),
        );
        assert_eq!(
            local_engine_from_dbus((id, details)),
            Err(EngineRowError::Place("moon".into()))
        );
    }
}

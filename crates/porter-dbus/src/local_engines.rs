//! The D-Bus shape of a local engine (`(sa{sv})`, a row of `Inference1.LocalEngines`): the
//! engine's id, then a vardict of its name, its state, its path when it is found, the slots it
//! would serve and, where they apply, its release, where it came from, how far an install has
//! come, why one failed and what could be installed. A client reads the typed [`LocalEngine`],
//! never the strings.

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
/// The vardict key of the engine's release (`s`), when it is known.
pub const ENGINE_KEY_VERSION: &str = "version";
/// The vardict key of where a found engine came from (`s`): `package`, `system`, `person` or
/// `pointed`.
pub const ENGINE_KEY_PLACE: &str = "place";
/// The vardict key of how far an install has come (`u`, thousandths): only while `installing`.
pub const ENGINE_KEY_PROGRESS: &str = "progress";
/// The vardict key of why an install failed (`s`, a plain sentence): only when `failed`.
pub const ENGINE_KEY_REASON: &str = "reason";
/// The vardict key of how many bytes installing it downloads (`t`), when that is known.
pub const ENGINE_KEY_SIZE: &str = "size";
/// The vardict key of the ways it can be installed (`as`: `all_users`, `just_me`); absent when
/// nothing can be installed for this computer.
pub const ENGINE_KEY_SCOPES: &str = "scopes";

/// The state word of an engine that has a program on this computer.
const FOUND: &str = "found";
/// The state word of an engine with no program, or whose path is not there.
const MISSING: &str = "missing";
/// The state word of an engine being installed now.
const INSTALLING: &str = "installing";
/// The state word of an engine whose install failed.
const FAILED: &str = "failed";

/// Whether an engine's program is on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// The program is there, at this path.
    Found(PathBuf),
    /// No path is set or discovered, or the path is not there.
    Missing,
    /// An install is under way, this many thousandths done.
    Installing(u32),
    /// The last install failed, for this plain reason.
    Failed(String),
}

/// Where a found engine came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        match self {
            Self::Package => "package",
            Self::System => "system",
            Self::Person => "person",
            Self::Pointed => "pointed",
        }
    }

    /// The place a word names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        [Self::Package, Self::System, Self::Person, Self::Pointed]
            .into_iter()
            .find(|place| place.slug() == text)
    }
}

/// What could be installed for an engine that is not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOffer {
    /// How many bytes it downloads, when that is known (a distro package's size is not).
    pub size: Option<u64>,
    /// The ways to install it, as the words `all_users` and `just_me`.
    pub scopes: Vec<String>,
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
    /// What could be installed, when it is not found and something can be.
    pub offer: Option<InstallOffer>,
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
        EngineState::Missing => (MISSING, None),
        EngineState::Installing(_) => (INSTALLING, None),
        EngineState::Failed(_) => (FAILED, None),
    };
    let progress = match engine.state {
        EngineState::Installing(thousandths) => Some(thousandths),
        _ => None,
    };
    let reason = match &engine.state {
        EngineState::Failed(reason) => Some(reason.clone()),
        _ => None,
    };
    let offer = engine.offer.as_ref();
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
        (
            ENGINE_KEY_PROGRESS,
            progress.and_then(|p| owned(Value::from(p))),
        ),
        (
            ENGINE_KEY_REASON,
            reason.and_then(|r| owned(Value::from(r))),
        ),
        (
            ENGINE_KEY_SIZE,
            offer
                .and_then(|o| o.size)
                .and_then(|size| owned(Value::from(size))),
        ),
        (
            ENGINE_KEY_SCOPES,
            offer.and_then(|o| owned(Value::Array(o.scopes.clone().into()))),
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
        INSTALLING => EngineState::Installing(number(&details, ENGINE_KEY_PROGRESS).unwrap_or(0)),
        FAILED => EngineState::Failed(text(&details, ENGINE_KEY_REASON).unwrap_or_default()),
        other => return Err(EngineRowError::State(other.to_owned())),
    };
    let slots =
        list(&details, ENGINE_KEY_SLOTS).ok_or(EngineRowError::Missing(ENGINE_KEY_SLOTS))?;
    let place = match text(&details, ENGINE_KEY_PLACE) {
        Ok(word) => Some(EnginePlace::from_slug(&word).ok_or(EngineRowError::Place(word))?),
        Err(_) => None,
    };
    let offer = list(&details, ENGINE_KEY_SCOPES).map(|scopes| InstallOffer {
        size: number(&details, ENGINE_KEY_SIZE),
        scopes,
    });
    Ok(LocalEngine {
        name: text(&details, ENGINE_KEY_NAME)?,
        id,
        state,
        slots,
        version: text(&details, ENGINE_KEY_VERSION).ok(),
        place,
        offer,
    })
}

/// A key's value as a number of this type, when it is one.
fn number<T: TryFrom<OwnedValue>>(details: &Details, key: &str) -> Option<T> {
    details
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| T::try_from(value).ok())
}

/// A key's value as a list of words, when it is one.
fn list(details: &Details, key: &str) -> Option<Vec<String>> {
    number(details, key)
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
            version: None,
            place: None,
            offer: None,
        }
    }

    #[test]
    fn a_row_round_trips_and_a_missing_engine_carries_no_path() {
        let found = LocalEngine {
            version: Some("b11669".into()),
            place: Some(EnginePlace::Person),
            ..engine(EngineState::Found(PathBuf::from("/usr/bin/llama-server")))
        };
        let missing = LocalEngine {
            offer: Some(InstallOffer {
                size: Some(30_497_889),
                scopes: vec!["all_users".into(), "just_me".into()],
            }),
            ..engine(EngineState::Missing)
        };
        let nothing_to_install = engine(EngineState::Missing);
        for engine in [found, missing, nothing_to_install] {
            let row = local_engine_to_dbus(&engine);
            assert_eq!(
                row.1.contains_key(ENGINE_KEY_PATH),
                matches!(engine.state, EngineState::Found(_))
            );
            assert_eq!(
                row.1.contains_key(ENGINE_KEY_SCOPES),
                engine.offer.is_some()
            );
            assert_eq!(local_engine_from_dbus(row), Ok(engine));
        }
    }

    #[test]
    fn an_install_under_way_and_one_that_failed_carry_their_progress_and_reason() {
        let installing = engine(EngineState::Installing(420));
        let failed = engine(EngineState::Failed(
            "The download didn't finish.".to_owned(),
        ));
        for engine in [installing, failed] {
            let row = local_engine_to_dbus(&engine);
            assert!(!row.1.contains_key(ENGINE_KEY_PATH));
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

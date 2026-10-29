//! The small builders of a [`KeySpec`] every row of a settings module uses.

use crate::Boxed;
use ds_settings::live::LiveError;
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Page, Section, WordLabels,
};

/// A key on the Accounts page at `path`, in `section`, hands off to agents.
pub fn spec(
    path: String,
    section: &str,
    label: String,
    help: &str,
    kind: KeyKind,
    default: toml::Value,
    exposure: Exposure,
) -> KeySpec {
    KeySpec {
        control: None,
        path: KeyPath(path),
        kind,
        default,
        label: Label(label),
        help: Help(help.to_owned()),
        page: Page::Accounts,
        section: Section(section.to_owned()),
        exposure,
        labels: Default::default(),
        unavailable: Default::default(),
        groups: Default::default(),
        places: Default::default(),
        note: None,
        agent: AgentSetting::HandsOff,
    }
}

/// The kind of a row that is a button.
pub fn action(name: &str, weight: ActionWeight) -> KeyKind {
    KeyKind::Live {
        action: LiveAction {
            label: ActionLabel(name.to_owned()),
            weight,
        },
    }
}

/// The value of a button.
pub fn off() -> toml::Value {
    toml::Value::Boolean(false)
}

/// `spec` with the words of its enum key labelled for a person.
pub fn labelled(mut spec: KeySpec, words: &[(&str, &str)]) -> KeySpec {
    spec.labels = WordLabels(
        words
            .iter()
            .map(|(word, label)| ((*word).to_owned(), (*label).to_owned()))
            .collect(),
    );
    spec
}

/// A read-out row whose one value is `word`, as a person reads it by `labels`.
pub fn readout(
    path: String,
    section: &str,
    label: &str,
    word: &str,
    labels: &[(&str, &str)],
) -> KeySpec {
    labelled(
        spec(
            path,
            section,
            label.to_owned(),
            "",
            KeyKind::Fixed {
                variant: word.to_owned(),
            },
            toml::Value::String(word.to_owned()),
            Exposure::Basic,
        ),
        labels,
    )
}

/// A write that is turned away with `why`.
pub fn refused<'a>(why: LiveError) -> Boxed<'a, Result<(), LiveError>> {
    Box::pin(async move { Err(why) })
}

/// A row that is a button: `name` on the button, `label` beside it.
pub fn button(
    path: String,
    section: &str,
    label: &str,
    help: &str,
    name: &str,
    weight: ActionWeight,
) -> KeySpec {
    spec(
        path,
        section,
        label.to_owned(),
        help,
        action(name, weight),
        off(),
        Exposure::Basic,
    )
}

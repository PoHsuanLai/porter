//! Converting porter's values to and from their D-Bus shapes. Parse at the boundary: a daemon
//! turns a `NeedArg` into a `Need` here and nowhere else.
//!
//! A need is its kind's slug plus a vardict whose entries are the need's fields by name, each
//! field in its serde form (`json_value` is the one conversion). A candidate is its account's
//! object path, its label, and a vardict of the rest by name: `account` (the exact id, because
//! the path's segment is lossy), `provider`, `subject`, `capability`, `restriction` and `grant`.

use crate::args::{CandidateArg, NeedArg};
use crate::json_value::{bad, from_vardict, to_vardict};
use crate::names::account_path;
use porter_core::{Candidate, CoreError, Need};
use serde_json::{Map, Value as Json};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

/// A need as its D-Bus argument.
pub fn need_to_dbus(need: &Need) -> NeedArg {
    let (kind, fields) = match serde_json::to_value(need) {
        Ok(Json::Object(mut envelope)) => (
            envelope.remove("kind"),
            envelope.remove("v").and_then(into_object),
        ),
        _ => (None, None),
    };
    // A need serialises to `{kind, v: {fields}}` (every need is a struct); the fallbacks are
    // unreachable and kept total so this stays infallible.
    let kind = kind.and_then(into_string).unwrap_or_default();
    (kind, to_vardict(&fields.unwrap_or_default()))
}

/// The need a D-Bus argument carries, or why it is not one.
pub fn need_from_dbus(arg: NeedArg) -> Result<Need, CoreError> {
    let (kind, details) = arg;
    let fields = from_vardict(&details)?;
    let envelope = Map::from_iter([
        ("kind".to_owned(), Json::String(kind)),
        ("v".to_owned(), Json::Object(fields)),
    ]);
    serde_json::from_value(Json::Object(envelope)).map_err(|e| bad(&format!("need: {e}")))
}

/// A candidate as its D-Bus shape.
pub fn candidate_to_dbus(candidate: &Candidate) -> CandidateArg {
    let mut fields = match serde_json::to_value(candidate) {
        Ok(Json::Object(fields)) => fields,
        _ => Map::new(),
    };
    let label = fields
        .remove("label")
        .and_then(into_string)
        .unwrap_or_default();
    let path = ObjectPath::try_from(account_path(&candidate.account))
        .map(OwnedObjectPath::from)
        .unwrap_or_else(|_| OwnedObjectPath::from(ObjectPath::from_static_str_unchecked("/")));
    (path, label, to_vardict(&fields))
}

/// The candidate a D-Bus value carries, or why it is not one.
pub fn candidate_from_dbus(arg: CandidateArg) -> Result<Candidate, CoreError> {
    let (path, label, details) = arg;
    let mut fields = from_vardict(&details)?;
    fields.insert("label".to_owned(), Json::String(label));
    let candidate: Candidate = serde_json::from_value(Json::Object(fields))
        .map_err(|e| bad(&format!("candidate: {e}")))?;
    match account_path(&candidate.account) == path.as_str() {
        true => Ok(candidate),
        false => Err(bad("candidate: the path is not the account's")),
    }
}

fn into_object(value: Json) -> Option<Map<String, Json>> {
    match value {
        Json::Object(fields) => Some(fields),
        _ => None,
    }
}

fn into_string(value: Json) -> Option<String> {
    match value {
        Json::String(text) => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_need_with_no_fields_is_an_empty_vardict() {
        let need = Need::Push(porter_core::need::PushNeed::new());
        let (kind, details) = need_to_dbus(&need);
        assert_eq!(kind, "push");
        assert!(details.is_empty());
        assert_eq!(need_from_dbus((kind, details)), Ok(need));
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let arg = ("teleport".to_owned(), Default::default());
        assert!(need_from_dbus(arg).is_err());
    }
}

//! A need to and from its D-Bus shape. Parse at the boundary: a daemon turns a `NeedArg` into a
//! `Need` here and nowhere else.
//!
//! A need is its kind's slug plus a vardict whose entries are the need's fields by name, each
//! field in its serde form (`json_value` is the one conversion).

use crate::Details;
use crate::json_value::{bad, from_vardict, into_object, into_string, to_vardict};
use porter_core::CoreError;
use porter_match::Need;
use serde_json::{Map, Value as Json};

/// A need (`(sa{sv})`): the kind's slug and its fields by name.
pub type NeedArg = (String, Details);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_need_with_no_fields_is_an_empty_vardict() {
        let need = Need::Push(porter_match::PushNeed::new());
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

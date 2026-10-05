//! The D-Bus shape of a legacy reference (`a{sv}`): the record's fields by name, with the
//! endpoints a list of records.

use crate::args::Details;
use crate::json_value::{bad, from_vardict, to_vardict};
use porter_core::CoreError;
use porter_core::wire::LegacyRef;
use serde_json::{Map, Value as Json};

/// A legacy reference as its D-Bus argument.
pub fn legacy_to_dbus(legacy: &LegacyRef) -> Details {
    match serde_json::to_value(legacy) {
        Ok(Json::Object(fields)) => to_vardict(&fields),
        // A struct always serializes to an object; the fallback keeps this infallible.
        _ => to_vardict(&Map::new()),
    }
}

/// The legacy reference a D-Bus argument carries, or why it is not one.
pub fn legacy_from_dbus(details: &Details) -> Result<LegacyRef, CoreError> {
    let fields = from_vardict(details)?;
    serde_json::from_value(Json::Object(fields)).map_err(|e| bad(&format!("legacy: {e}")))
}

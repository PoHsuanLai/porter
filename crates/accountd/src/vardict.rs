//! A vardict as the JSON record it carries: the way back of `porter_dbus::to_vardict`, which
//! porter-dbus does not export (its `from_vardict` is crate-private). This is the same
//! conversion for the one place accountd reads a record a daemon sent (`Peer.ReportLocal`'s
//! claims); it goes when porter-dbus exports its own (FINDINGS).

use porter_dbus::Details;
use serde_json::{Map, Number, Value as Json};
use zbus::zvariant::Value;

/// The record `details` carries, or why it is not one porter's values use.
pub(crate) fn record_of(details: &Details) -> Result<Map<String, Json>, String> {
    details
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_of(value)?)))
        .collect()
}

fn json_of(value: &Value<'_>) -> Result<Json, String> {
    match value {
        Value::Value(inner) => json_of(inner),
        Value::Bool(b) => Ok(Json::Bool(*b)),
        Value::U8(n) => Ok(Json::from(*n)),
        Value::I16(n) => Ok(Json::from(*n)),
        Value::U16(n) => Ok(Json::from(*n)),
        Value::I32(n) => Ok(Json::from(*n)),
        Value::U32(n) => Ok(Json::from(*n)),
        Value::I64(n) => Ok(Json::from(*n)),
        Value::U64(n) => Ok(Json::from(*n)),
        Value::F64(f) => Number::from_f64(*f)
            .map(Json::Number)
            .ok_or_else(|| "a number that is not finite".to_owned()),
        Value::Str(s) => Ok(Json::String(s.as_str().to_owned())),
        Value::ObjectPath(p) => Ok(Json::String(p.as_str().to_owned())),
        Value::Array(items) => items
            .inner()
            .iter()
            .map(json_of)
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
        Value::Dict(dict) => dict
            .iter()
            .map(|(key, value)| Ok((key_text(key)?, json_of(value)?)))
            .collect::<Result<Map<_, _>, String>>()
            .map(Json::Object),
        _ => Err("a value of a type porter does not use".to_owned()),
    }
}

fn key_text(key: &Value<'_>) -> Result<String, String> {
    match key {
        Value::Str(s) => Ok(s.as_str().to_owned()),
        Value::Value(inner) => key_text(inner),
        _ => Err("a record key that is not a string".to_owned()),
    }
}

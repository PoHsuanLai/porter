//! The one conversion between a serde JSON tree and a D-Bus value, so a `Need` or a
//! `Candidate` is a vardict whose entries are its fields by name. A string is `s`, an integer
//! `x` (`t` above `i64::MAX`), a list `av` and a nested record `a{sv}`; a variant inside a
//! list or record is wrapped once (`v`), as D-Bus requires. `null` has no D-Bus form: an
//! absent key stands for it, and porter's values hold no `Option` inside a list.

use porter_core::CoreError;
use serde_json::{Map, Number, Value as Json};
use zbus::zvariant::{Array, Dict, OwnedValue, Signature, Value};

/// A JSON value as a D-Bus value; `None` for `null`.
pub fn to_value(json: &Json) -> Option<Value<'static>> {
    match json {
        Json::Null => None,
        Json::Bool(b) => Some(Value::Bool(*b)),
        Json::Number(n) => Some(number_to_value(n)),
        Json::String(s) => Some(Value::from(s.clone())),
        Json::Array(items) => {
            let mut array = Array::new(&Signature::Variant);
            items
                .iter()
                .filter_map(to_value)
                .for_each(|v| append(&mut array, v));
            Some(Value::Array(array))
        }
        Json::Object(fields) => {
            let mut dict = Dict::new(&Signature::Str, &Signature::Variant);
            fields.iter().for_each(|(key, value)| {
                if let Some(v) = to_value(value) {
                    // The key and value signatures are the dict's own, so this cannot fail.
                    let _ = dict.append(Value::from(key.clone()), Value::Value(Box::new(v)));
                }
            });
            Some(Value::Dict(dict))
        }
    }
}

fn append(array: &mut Array<'static>, value: Value<'static>) {
    // The element signature is `v`, so every element fits.
    let _ = array.append(Value::Value(Box::new(value)));
}

fn number_to_value(n: &Number) -> Value<'static> {
    match (n.as_i64(), n.as_u64()) {
        (Some(i), _) => Value::I64(i),
        (None, Some(u)) => Value::U64(u),
        (None, None) => Value::F64(n.as_f64().unwrap_or_default()),
    }
}

/// A D-Bus value as a JSON value, or why it is not one porter's values use.
pub fn from_value(value: &Value<'_>) -> Result<Json, CoreError> {
    match value {
        Value::Value(inner) => from_value(inner),
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
            .ok_or_else(|| bad("a number that is not finite")),
        Value::Str(s) => Ok(Json::String(s.as_str().to_owned())),
        Value::ObjectPath(p) => Ok(Json::String(p.as_str().to_owned())),
        Value::Array(items) => items
            .inner()
            .iter()
            .map(from_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
        Value::Dict(dict) => dict
            .iter()
            .map(|(key, value)| Ok((key_text(key)?, from_value(value)?)))
            .collect::<Result<Map<_, _>, CoreError>>()
            .map(Json::Object),
        _ => Err(bad("a value of a type porter does not use")),
    }
}

fn key_text(key: &Value<'_>) -> Result<String, CoreError> {
    match key {
        Value::Str(s) => Ok(s.as_str().to_owned()),
        Value::Value(inner) => key_text(inner),
        _ => Err(bad("a record key that is not a string")),
    }
}

/// The entries of a record as a vardict; `null` fields are left out.
pub fn to_vardict(fields: &Map<String, Json>) -> std::collections::HashMap<String, OwnedValue> {
    fields
        .iter()
        .filter_map(|(key, value)| {
            let owned = OwnedValue::try_from(to_value(value)?).ok()?;
            Some((key.clone(), owned))
        })
        .collect()
}

/// A vardict as the record it carries.
pub fn from_vardict(
    details: &std::collections::HashMap<String, OwnedValue>,
) -> Result<Map<String, Json>, CoreError> {
    details
        .iter()
        .map(|(key, value)| Ok((key.clone(), from_value(value)?)))
        .collect()
}

pub(crate) fn bad(why: &str) -> CoreError {
    CoreError::MalformedFrame(format!("D-Bus argument: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn json() -> impl Strategy<Value = Json> {
        // No `null` (it has no D-Bus form) and no floats (porter stores none).
        let leaf = prop_oneof![
            any::<i64>().prop_map(Json::from),
            any::<u64>().prop_map(Json::from),
            "[a-z_.]{0,12}".prop_map(Json::from),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Json::Array),
                prop::collection::btree_map("[a-z_]{1,8}", inner, 0..4)
                    .prop_map(|m| Json::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #[test]
        fn a_json_tree_survives_the_bus_value(tree in json()) {
            let value = to_value(&tree).expect("not null");
            prop_assert_eq!(from_value(&value).expect("converts back"), tree);
        }
    }

    #[test]
    fn null_has_no_value_and_a_null_field_is_left_out() {
        assert!(to_value(&Json::Null).is_none());
        let fields: Map<String, Json> = serde_json::from_str(r#"{"a":1,"b":null}"#).expect("json");
        let vardict = to_vardict(&fields);
        assert_eq!(vardict.len(), 1);
        assert_eq!(
            from_vardict(&vardict).expect("back"),
            serde_json::from_str::<Map<String, Json>>(r#"{"a":1}"#).expect("json")
        );
    }

    #[test]
    fn a_value_porter_does_not_use_is_refused() {
        let value = Value::from(zbus::zvariant::Signature::Str);
        assert!(from_value(&value).is_err());
    }
}

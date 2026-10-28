//! A candidate to and from its D-Bus shape: its account's object path, its label, and a vardict of
//! the rest by name: `account` (the exact id, because the path's segment is lossy), `provider`,
//! `subject`, `capability`, `restriction` and `grant`.

use super::account_path;
use crate::Details;
use crate::json_value::{bad, from_vardict, into_string, to_vardict};
use porter_core::{Candidate, CoreError};
use serde_json::{Map, Value as Json};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

/// A candidate (`(osa{sv})`): the account's object path, its label, and the rest by name.
pub type CandidateArg = (OwnedObjectPath, String, Details);

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

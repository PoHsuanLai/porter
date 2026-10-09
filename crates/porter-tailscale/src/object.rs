//! Reading a JSON object. serde's derived structs also read a JSON array, field by field, which
//! is never what Tailscale sends: only an object is an answer.

use crate::error::TailscaleError;
use serde::de::DeserializeOwned;

/// `T` from a body that is a JSON object, else `Malformed`.
pub(crate) fn object<T: DeserializeOwned>(body: &[u8]) -> Result<T, TailscaleError> {
    let first = body.iter().find(|b| !b.is_ascii_whitespace());
    match first {
        Some(b'{') => serde_json::from_slice(body).map_err(|_| TailscaleError::Malformed),
        _ => Err(TailscaleError::Malformed),
    }
}

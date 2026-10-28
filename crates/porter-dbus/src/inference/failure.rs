//! How inferd's own refusals read on the caller's side: the no-allowed-place reply and a refusal of
//! a computer call.

/// The prefix of the error inferd answers a call that named its `places` with when none of them
/// can serve it: `org.quire.Inference1.Error.NoAllowedPlace.<Reason>`, with `NotReady`,
/// `FloorRefused`, `ModelNotOffered` or `NoneCapable` as the reason. The reply's body is two
/// strings: the words for a person, and the kind of place outside the set that could have served
/// (`this_computer`, `own_computer`, `cloud_account`; empty when none could).
pub const PLACE_ERROR_PREFIX: &str = "org.quire.Inference1.Error.NoAllowedPlace.";

/// What a no-allowed-place reply says, as the text the daemon wrote: the reason's name (the error
/// name's last segment) and, when a place outside the set could have served, its kind's slug.
/// `None` for any other error.
pub fn place_refusal_of(error: &zbus::Error) -> Option<(String, Option<String>)> {
    let zbus::Error::MethodError(name, _, message) = error else {
        return None;
    };
    let reason = name.as_str().strip_prefix(PLACE_ERROR_PREFIX)?;
    let would_need = message
        .body()
        .deserialize::<(String, String)>()
        .ok()
        .map(|(_, kind)| kind)
        .filter(|kind| !kind.is_empty());
    Some((reason.to_owned(), would_need))
}

/// What a refusal of a computer call says (`org.quire.Inference1.Error.Computer.<Name>`): the
/// error's name after the prefix and the plain sentence the daemon wrote for the person (empty
/// when it sent none). `None` for any other error.
pub fn computer_refusal_of(error: &zbus::Error) -> Option<(String, String)> {
    let zbus::Error::MethodError(name, words, _) = error else {
        return None;
    };
    let reason = name.as_str().strip_prefix(super::COMPUTER_ERROR_PREFIX)?;
    Some((reason.to_owned(), words.clone().unwrap_or_default()))
}

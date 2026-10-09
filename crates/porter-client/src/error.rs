//! porter-client's errors.

use porter_core::AccountId;
use porter_core::wire::Refusal;
#[cfg(feature = "infer")]
use porter_infer::{InferRefusal, NoPlaceReason, PlaceKind, SessionError};

/// Why a transport could not carry a request. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// No daemon answered.
    #[error("no account service reachable")]
    Unreachable,
    /// The connection closed mid-request.
    #[error("connection closed")]
    Closed,
    /// The daemon refused the caller: the bus's `AccessDenied`, with the daemon's own text (its
    /// caller table does not name this program, or the app holds no grant for the call). Asking
    /// again does not help; the person or the packager must change who may call.
    #[error("refused by the daemon: {0}")]
    Denied(String),
    /// The other side sent something that is not porter's protocol.
    #[error("malformed reply: {0}")]
    Malformed(String),
    /// The call named the places it may run (`OpenOptions::places`) and none of them can serve
    /// it: the reason, and the kind of place outside the set that could have (feature `infer`).
    #[cfg(feature = "infer")]
    #[error("none of the allowed places can serve this ({})", .reason.slug())]
    NoAllowedPlace {
        /// Why nothing allowed can serve.
        reason: NoPlaceReason,
        /// The kind of place outside the set that could have served it, if there is one.
        would_need: Option<PlaceKind>,
    },
}

/// Why a client call failed; each variant is something an app can show or act on. More reasons
/// may be added (and `InferRefused` exists only with the `infer` feature): match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// The request did not arrive or the reply did not come back. This also carries inferd's
    /// answer that none of the places a call allowed can serve it
    /// ([`TransportError::NoAllowedPlace`], feature `infer`); [`ClientError::no_allowed_place`]
    /// reads it.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// accountd refused.
    #[error("refused: {}", why(*.0))]
    Refused(Refusal),
    /// `add_account`: the person signed in to an account that was already here, so nothing was
    /// added. It is that account; ask for a grant of it (`find`, then `request_grant`).
    #[error("that account is already added: {0}")]
    AlreadyAdded(AccountId),
    /// inferd refused (feature `infer`).
    #[cfg(feature = "infer")]
    #[error(transparent)]
    InferRefused(#[from] InferRefusal),
    /// The reply does not answer the request (a daemon of another version).
    #[error("reply does not answer the request")]
    Mismatched,
}

impl ClientError {
    /// The call named the places it may run (`OpenOptions::places`) and none of them can serve
    /// it: why, and the kind of place outside the set that could have served it, if there is one,
    /// so an app can offer to turn it on (feature `infer`).
    #[cfg(feature = "infer")]
    pub fn no_allowed_place(&self) -> Option<(NoPlaceReason, Option<PlaceKind>)> {
        match self {
            ClientError::Transport(TransportError::NoAllowedPlace { reason, would_need }) => {
                Some((*reason, *would_need))
            }
            _ => None,
        }
    }
}

/// What a refusal says in words (the enum has no `Display` of its own; the match names every
/// refusal, so a new one cannot go unsaid).
fn why(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::Dismissed => "the person closed the window",
        Refusal::Denied => "the person said no",
        Refusal::NoFittingAccount => "no account fits",
        Refusal::UnknownGrant => "that permission is not this app's, or is gone",
        Refusal::AudienceNotGranted => "the permission does not cover that address",
        Refusal::NeedsReauth => "the account must be signed in again",
        Refusal::Unavailable => "the account or the saved secrets cannot be reached",
        Refusal::EndpointNotGranted => "that address is not one of the account's",
        Refusal::NoLauncher => "nothing is there to sign the agent in",
    }
}

#[cfg(feature = "infer")]
impl From<SessionError> for TransportError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Closed => TransportError::Closed,
            SessionError::Malformed(why) => TransportError::Malformed(why),
        }
    }
}

#[cfg(all(test, feature = "infer"))]
mod tests {
    use super::*;

    #[test]
    fn the_place_refusal_is_one_variant_and_the_client_error_carries_it() {
        let refusal = TransportError::NoAllowedPlace {
            reason: NoPlaceReason::NotReady,
            would_need: Some(PlaceKind::CloudAccount),
        };
        let error = ClientError::from(refusal.clone());
        assert_eq!(error, ClientError::Transport(refusal));
        assert_eq!(
            error.no_allowed_place(),
            Some((NoPlaceReason::NotReady, Some(PlaceKind::CloudAccount)))
        );
        assert_eq!(ClientError::Mismatched.no_allowed_place(), None);
        assert_eq!(
            ClientError::from(TransportError::Closed).no_allowed_place(),
            None
        );
        assert_eq!(
            error.to_string(),
            "none of the allowed places can serve this (not_ready)"
        );
    }
}

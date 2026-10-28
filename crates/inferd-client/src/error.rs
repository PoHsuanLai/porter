//! inferd-client's errors.

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
    /// This link does not carry that call: the computers and the guests are inferd's, over the
    /// session bus, and a link with no inferd (the app hosting the core itself) has
    /// neither.
    #[error("this link does not carry that call")]
    Unsupported,
    /// inferd refused a call about computers or guests. An app words the refusal from `reason`;
    /// `words` is the text inferd sent with it, for an app that has none of its own.
    #[error("inferd refused a call about computers or guests: {reason}")]
    Computer {
        /// Why, as a word an app can act on.
        reason: ComputerReason,
        /// The text inferd sent with the refusal.
        words: String,
    },
    /// inferd would not start or stop a model: the model is not one it runs, or its engine did
    /// not come up. The payload is the text inferd sent with the refusal.
    #[error("inferd would not start or stop the model: {0}")]
    NotRun(String),
    /// The call named the places it may run (`OpenOptions::places`) and none of them can serve
    /// it: the reason, and the kind of place outside the set that could have (feature `infer`).
    #[error("none of the allowed places can serve this ({})", .reason.slug())]
    NoAllowedPlace {
        /// Why nothing allowed can serve.
        reason: NoPlaceReason,
        /// The kind of place outside the set that could have served it, if there is one.
        would_need: Option<PlaceKind>,
    },
}

/// Why inferd refused a call about computers or guests. More reasons may be added: match with a
/// wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ComputerReason {
    /// The name is empty, too long or has control characters.
    #[error("bad name")]
    BadName,
    /// No model was given.
    #[error("no models")]
    NoModels,
    /// More models, or more computers, than may be kept.
    #[error("too many")]
    TooMany,
    /// A computer of that name is already there.
    #[error("already there")]
    AlreadyThere,
    /// The assistant does not know a model that was named.
    #[error("unknown model")]
    UnknownModel,
    /// A model that cannot be used from another computer.
    #[error("model not usable")]
    ModelNotUsable,
    /// A model that is already set up.
    #[error("model taken")]
    ModelTaken,
    /// How to reach a model cannot be used.
    #[error("bad address")]
    BadAddress,
    /// A key that cannot be used.
    #[error("bad key")]
    BadKey,
    /// The change could not be saved.
    #[error("not saved")]
    NotSaved,
    /// There is no computer of that name.
    #[error("not there")]
    NotThere,
    /// The computer was written in the settings file by hand.
    #[error("added by hand")]
    AddedByHand,
    /// Adding computers is not set up in this daemon.
    #[error("unavailable")]
    Unavailable,
    /// The computer is not one of the person's own on Tailscale.
    #[error("not on Tailscale")]
    NotOnTailscale,
    /// The computer does not lend its models right now.
    #[error("not answering")]
    NotAnswering,
    /// The computer is not asking to use this computer's models.
    #[error("not asking")]
    NotAsking,
    /// Too many computers are asking at once.
    #[error("too many asking")]
    TooManyAsking,
    /// A reason this client does not know yet, as the daemon named it.
    #[error("{0}")]
    Other(String),
}

impl ComputerReason {
    /// The reason a daemon's error name stands for (the part after the `Computer.` prefix).
    pub fn from_name(name: &str) -> Self {
        match name {
            "BadName" => Self::BadName,
            "NoModels" => Self::NoModels,
            "TooMany" => Self::TooMany,
            "AlreadyThere" => Self::AlreadyThere,
            "UnknownModel" => Self::UnknownModel,
            "ModelNotUsable" => Self::ModelNotUsable,
            "ModelTaken" => Self::ModelTaken,
            "BadAddress" => Self::BadAddress,
            "BadKey" => Self::BadKey,
            "NotSaved" => Self::NotSaved,
            "NotThere" => Self::NotThere,
            "AddedByHand" => Self::AddedByHand,
            "Unavailable" => Self::Unavailable,
            "NotOnTailscale" => Self::NotOnTailscale,
            "NotAnswering" => Self::NotAnswering,
            "NotAsking" => Self::NotAsking,
            "TooManyAsking" => Self::TooManyAsking,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// Why a client call failed; each variant is something an app can show or act on. More reasons
/// may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// The request did not arrive or the reply did not come back. This also carries inferd's
    /// answer that none of the places a call allowed can serve it
    /// ([`TransportError::NoAllowedPlace`], feature `infer`); [`ClientError::no_allowed_place`]
    /// reads it.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// inferd refused.
    #[error(transparent)]
    InferRefused(#[from] InferRefusal),
}

impl ClientError {
    /// The call named the places it may run (`OpenOptions::places`) and none of them can serve
    /// it: why, and the kind of place outside the set that could have served it, if there is one,
    /// so an app can offer to turn it on.
    pub fn no_allowed_place(&self) -> Option<(NoPlaceReason, Option<PlaceKind>)> {
        match self {
            ClientError::Transport(TransportError::NoAllowedPlace { reason, would_need }) => {
                Some((*reason, *would_need))
            }
            _ => None,
        }
    }
}
impl From<SessionError> for TransportError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Closed => TransportError::Closed,
            SessionError::Malformed(why) => TransportError::Malformed(why),
            // a variant a newer porter adds: the session is treated as closed
            _ => TransportError::Closed,
        }
    }
}

#[cfg(test)]
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
        assert_eq!(
            ClientError::Transport(TransportError::Unreachable).no_allowed_place(),
            None
        );
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

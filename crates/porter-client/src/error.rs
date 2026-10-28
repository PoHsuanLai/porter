//! porter-client's failures that belong to no one domain: why a call to a service did not
//! complete. Each domain's error holds one beside the reasons it gives of its own. Nothing here
//! carries a sentence for a person; the apps word them.

/// Why a call did not complete, whatever the call was. More reasons may be added: match with a
/// wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LinkError {
    /// No daemon answered: nobody owns the name, or there is no bus.
    #[error("unreachable")]
    Unreachable,
    /// The connection closed mid-call.
    #[error("closed")]
    Closed,
    /// The daemon refused the caller (the bus's `AccessDenied`): its caller table does not name
    /// this program, or the call is not one it may make. Asking again does not help.
    #[error("denied")]
    Denied,
    /// The daemon refused an argument (an id, a name or a program that is not one).
    #[error("invalid")]
    Invalid,
    /// The other side sent something that is not porter's protocol.
    #[error("malformed {0:?}")]
    Malformed(Malformed),
}

/// What the other side got wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Malformed {
    /// A reply that does not answer the call.
    Reply,
    /// One row of a list that does not say what the interface promises.
    Row,
    /// A signal whose arguments are not what the interface promises.
    Signal,
    /// An id or a name that fails its rule.
    Id,
    /// A descriptor that cannot be read as what it should hold.
    Descriptor,
    /// A sheet's `Response` that is not what the Request interface promises.
    Sheet,
    /// A failure this client does not know.
    UnknownFailure,
}

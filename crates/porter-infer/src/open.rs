//! What a client may attach when it opens a session, beside the need, the class and the tier.

use crate::ids::Traceparent;
use serde::{Deserialize, Serialize};

/// The options of `Inference1.Open`, `Prepare` and `Availability` (the `options` dictionary on the
/// bus, the first frame's options on the socket). Today one key is reserved; an unknown key is
/// ignored by inferd, so a newer client works with an older daemon.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpenOptions {
    /// The caller's trace, so a span started in companiond continues in inferd and one task is
    /// one trace. When absent, inferd starts its own root.
    pub traceparent: Option<Traceparent>,
}

//! Units and counts: every number that means something has a type.

use serde::{Deserialize, Serialize};

macro_rules! unit {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

unit!(
    /// A number of model tokens (context window, output limit, usage).
    Tokens(u32)
);
unit!(
    /// The length of an embedding vector.
    Dims(u32)
);
unit!(
    /// Image pixels along one side.
    Px(u32)
);
unit!(
    /// A plain count of things (documents, users, languages).
    Count(u32)
);
unit!(
    /// A size in bytes.
    Bytes(u64)
);
unit!(
    /// Seconds since the Unix epoch, UTC. Instants on the wire and in stores; the clock that
    /// makes them is injected (`porter_service::Clock`).
    UnixSeconds(i64)
);
unit!(
    /// Money in millionths of a US dollar: spend caps and price tables stay integers.
    MicroUsd(u64)
);
unit!(
    /// Thousandths of a whole: thresholds such as the spend warning at 800.
    Permille(u32)
);

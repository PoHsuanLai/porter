//! What a client may attach when it opens a session, beside the need, the class and the tier.

use crate::asker::Asker;
use crate::choice::ModelRef;
use crate::ids::Traceparent;
use crate::place::PlaceId;
use porter_capability::ModelId;
use porter_core::consent::Usage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The options of `Inference1.Open`, `Prepare` and `Availability` (the `options` dictionary on the
/// bus, the first frame's options on the socket). Today one key is reserved; an unknown key is
/// ignored by inferd, so a newer client works with an older daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenOptions {
    /// The caller's trace, so a span started in companiond continues in inferd and one task is
    /// one trace. When absent, inferd starts its own root.
    pub traceparent: Option<Traceparent>,
    /// Whether a person is waiting on the session (`interactive`) or nobody is (`background`):
    /// what inferd asks accountd's `Verdicts` with. Absent means `Interactive`, at the reader
    /// ([`OpenOptions::usage_or_default`]); an unknown slug is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// The places the session may run, in the caller's order of preference (`places` on the bus).
    /// Only docket's companion, reader and intents daemons may send it; inferd routes inside the
    /// set and never outside it. Absent means routing as it always was; present and empty is a
    /// set with nothing in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places: Option<Vec<PlaceId>>,
    /// The model to use at a place of the allowed set (`place_models` on the bus). A place
    /// with no entry uses the usual choice.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub place_models: BTreeMap<PlaceId, ModelId>,
    /// Whose turn the session carries (`asker` on the bus). Only docket's daemons may send it.
    /// Absent means the person ([`OpenOptions::asker_or_default`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asker: Option<Asker>,
    /// This session's own named model (`model` on the bus, `<account>/<model>`): a helper's
    /// pick. It is treated exactly like a model the person named for the slot: never
    /// overridden, a refusal names it, and no other model stands in. Only docket's daemons may
    /// send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
}

impl OpenOptions {
    /// These options carrying the caller's trace, so the daemon's spans continue it.
    pub fn with_traceparent(self, traceparent: Traceparent) -> Self {
        Self {
            traceparent: Some(traceparent),
            ..self
        }
    }

    /// These options limited to `places`, in this order of preference.
    pub fn with_places(self, places: Vec<PlaceId>) -> Self {
        Self {
            places: Some(places),
            ..self
        }
    }

    /// These options naming `model` for `place`.
    pub fn with_place_model(mut self, place: PlaceId, model: ModelId) -> Self {
        self.place_models.insert(place, model);
        self
    }

    /// These options with the session's usage named.
    pub fn with_usage(self, usage: Usage) -> Self {
        Self {
            usage: Some(usage),
            ..self
        }
    }

    /// These options for a turn asked by `asker`.
    pub fn with_asker(self, asker: Asker) -> Self {
        Self {
            asker: Some(asker),
            ..self
        }
    }

    /// These options naming `model` as the session's own model.
    pub fn with_model(self, model: ModelRef) -> Self {
        Self {
            model: Some(model),
            ..self
        }
    }

    /// Whose turn the session carries: the one named, else the person.
    pub fn asker_or_default(&self) -> Asker {
        self.asker.clone().unwrap_or(Asker::Person)
    }

    /// The usage the session is for: the one named, else `Interactive`.
    pub fn usage_or_default(&self) -> Usage {
        self.usage.unwrap_or(Usage::Interactive)
    }
}

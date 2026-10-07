//! The routing table the app gives, and the choice over it: a row says which engines serve a
//! kind of need at a tier, in the app's order of preference; the policy's floors and the keys
//! decide which of them may. Nothing is chosen by a default: a need or tier with no row is
//! `Unavailable`.

use super::engine::{Engine, EngineError, EngineId, KeyUse};
use super::keys::KeySource;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{Billing, DataClass, GrantId, Need, Tier};
use porter_infer::{InferRefusal, Policy, RouteAsk, RouteCandidate, Slot, SpendVerdict, admit};

/// One row: these engines, most preferred first, serve `slot` at `tier`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The kind of need ([`Slot::Text`] for a language model).
    pub slot: Slot,
    /// The tier the app asks for.
    pub tier: Tier,
    /// The engines, by id; the first one the policy and the keys admit answers.
    pub engines: Vec<EngineId>,
}

/// The slot a need is served from; `None` for a need this host does not serve (only a language
/// model is: embeddings, speech and computer use are `Unsupported`).
pub(crate) fn slot_of(need: &Need) -> Option<Slot> {
    match need {
        Need::Llm(_) => Some(Slot::Text),
        _ => None,
    }
}

/// The engines and the rows over them, checked against each other.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    engines: Vec<Engine>,
    rows: Vec<Route>,
}

impl Table {
    /// The table, or why it is not one: an engine given twice, a row naming an engine that was
    /// not given, a key that would cross the network in clear text.
    pub(crate) fn new(engines: Vec<Engine>, rows: Vec<Route>) -> Result<Self, EngineError> {
        for (index, engine) in engines.iter().enumerate() {
            if engines[..index].iter().any(|seen| seen.id == engine.id) {
                return Err(EngineError::Duplicate(engine.id.clone()));
            }
            let clear = engine.url.scheme == super::engine::Scheme::Http;
            if engine.key == KeyUse::Bearer && clear && !engine.url.is_loopback() {
                return Err(EngineError::KeyInTheClear(engine.id.clone()));
            }
        }
        let unknown = rows
            .iter()
            .flat_map(|row| &row.engines)
            .find(|id| !engines.iter().any(|engine| &engine.id == *id));
        match unknown {
            Some(id) => Err(EngineError::Unknown(id.clone())),
            None => Ok(Self { engines, rows }),
        }
    }

    fn engine(&self, id: &EngineId) -> Option<&Engine> {
        self.engines.iter().find(|engine| &engine.id == id)
    }

    /// The engines of the row for `slot` and `tier`, in the row's order.
    fn row(&self, slot: Slot, tier: Tier) -> Vec<&Engine> {
        self.rows
            .iter()
            .find(|row| row.slot == slot && row.tier == tier)
            .map(|row| {
                row.engines
                    .iter()
                    .filter_map(|id| self.engine(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The engine that answers a session for `need`, `class` and `tier`: the first of the row
    /// that porter-infer's `admit` (the hard rules, in the one place they are) lets through, with
    /// an engine that wants a key admitted only when `keys` has one.
    pub(crate) async fn choose(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        policy: &Policy,
        keys: &impl KeySource,
    ) -> Result<&Engine, InferRefusal> {
        let slot = slot_of(need).ok_or(InferRefusal::Unsupported)?;
        let row = self.row(slot, tier);
        let mut candidates = Vec::with_capacity(row.len());
        for engine in &row {
            let permission = match (engine.key, engine.key_held(keys).await) {
                (KeyUse::Bearer, false) => Verdict::Ask,
                _ => granted(),
            };
            candidates.push(RouteCandidate {
                account: engine.account.clone(),
                model: engine.model.clone(),
                locality: engine.locality.clone(),
                billing: Billing::Free,
                tier: porter_infer::TierChoice::Chosen,
                permission,
                spend: SpendVerdict::Within,
            });
        }
        let admitted = admit(RouteAsk { class }, &candidates, policy)?;
        admitted
            .first()
            .and_then(|first| {
                candidates
                    .iter()
                    .position(|candidate| std::ptr::eq(candidate, *first))
            })
            .and_then(|index| row.get(index).copied())
            .ok_or(InferRefusal::Unavailable)
    }
}

/// What an engine the app configured carries as consent: the app's own say-so, always.
fn granted() -> Verdict {
    match GrantId::parse("in-process") {
        Ok(grant) => Verdict::Granted {
            grant,
            scope: GrantScope::Always,
        },
        Err(_) => Verdict::Denied,
    }
}

impl Engine {
    /// Whether the key source has a key for this engine's account right now (the key is dropped
    /// at once).
    async fn key_held(&self, keys: &impl KeySource) -> bool {
        keys.key(&self.account).await.is_some()
    }
}

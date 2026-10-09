//! Routing inside a set of places. A caller that names the places a session may run (docket's
//! `places` option) is routed in them and nowhere else: each place in the caller's order is tried
//! with the usual rules (the floor, local-only, consent, spend, the person's picks), the first one
//! that can serve wins, and when none can the answer names one reason and, if a place outside the
//! set could have served, the kind of place it would take.
//!
//! Pure over the list of models with the place each is at and whether it can serve now; the
//! engines build that list.

use super::{Decided, Listed, choose, fits};
use porter_core::{DataClass, ModelId, Need, Tier};
use porter_infer::{
    AutoPolicy, InferRefusal, NoPlaceReason, PickRefusal, PlaceId, PlaceRefusal, Policy, RouteAsk,
    RouteCandidate, TierChoice, TierMap, admit,
};
use std::collections::BTreeMap;

/// The places a session may run, in order of preference, and the model to use at some of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowed {
    order: Vec<PlaceId>,
    pins: BTreeMap<PlaceId, ModelId>,
}

impl Allowed {
    /// The places in this order (a place named twice stays where it first was) with these pins.
    pub fn new(places: Vec<PlaceId>, pins: BTreeMap<PlaceId, ModelId>) -> Self {
        let mut order: Vec<PlaceId> = Vec::new();
        for place in places {
            if !order.contains(&place) {
                order.push(place);
            }
        }
        Self { order, pins }
    }

    /// The places, in order of preference.
    pub fn places(&self) -> &[PlaceId] {
        &self.order
    }

    /// The model pinned for `place`, if any.
    pub fn pin(&self, place: &PlaceId) -> Option<&ModelId> {
        self.pins.get(place)
    }

    /// Whether `place` is in the set.
    pub fn contains(&self, place: &PlaceId) -> bool {
        self.order.contains(place)
    }
}

/// Whether a model can take a request now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serving {
    /// It is up, coming up, or a request starts it.
    Now,
    /// It is not: weights to fetch, an engine that failed, an account to sign in again.
    NotNow,
}

/// A model, where it is, and whether it can serve now.
#[derive(Debug, Clone)]
pub struct Placed {
    /// The model as routing sees it.
    pub listed: Listed,
    /// The place it is served from.
    pub place: PlaceId,
    /// Whether it can serve now.
    pub serving: Serving,
}

/// Why a request could not be routed inside the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unplaced {
    /// A place could serve but a rule the person set refuses it (consent, spend, ...): the
    /// refusal is the usual one.
    Other(PickRefusal),
    /// Nothing in the set can serve, for this reason.
    NoPlace(PlaceRefusal),
}

impl Unplaced {
    /// The refusal a caller that knows nothing of places is told.
    pub fn into_pick(self) -> PickRefusal {
        match self {
            Unplaced::Other(refusal) => refusal,
            Unplaced::NoPlace(_) => InferRefusal::Unavailable.into(),
        }
    }
}

/// What one place came to.
enum Outcome {
    Chosen(Box<Decided>),
    Nothing(NoPlaceReason),
    Other(PickRefusal),
}

/// What the routing rules and the caller's answers are asked with.
#[derive(Debug, Clone, Copy)]
pub struct Rules<'a> {
    /// The person's hard rules.
    pub policy: &'a Policy,
    /// The person's picks.
    pub tiers: &'a TierMap,
    /// What Automatic may do.
    pub auto: AutoPolicy,
}

fn route_of(placed: &Placed) -> RouteCandidate {
    let card = &placed.listed.card;
    RouteCandidate {
        account: card.account.clone(),
        model: card.model.clone(),
        locality: card.locality.clone(),
        billing: card.billing.clone(),
        tier: TierChoice::Other,
        permission: placed.listed.permission.clone(),
        spend: placed.listed.spend,
    }
}

/// Tries one place.
fn attempt(
    need: &Need,
    class: DataClass,
    tier: Tier,
    here: &[&Placed],
    pin: Option<&ModelId>,
    rules: Rules<'_>,
) -> Outcome {
    if let Some(model) = pin
        && !here.iter().any(|one| one.listed.card.model == *model)
    {
        return Outcome::Nothing(NoPlaceReason::ModelNotOffered);
    }
    let fitting: Vec<&Placed> = here
        .iter()
        .copied()
        .filter(|one| fits(need, &one.listed.card))
        .filter(|one| pin.is_none_or(|model| one.listed.card.model == *model))
        .collect();
    if fitting.is_empty() {
        return Outcome::Nothing(NoPlaceReason::NoneCapable);
    }
    let servable: Vec<&Placed> = fitting
        .into_iter()
        .filter(|one| one.serving == Serving::Now)
        .collect();
    if servable.is_empty() {
        return Outcome::Nothing(NoPlaceReason::NotReady);
    }
    let routes: Vec<RouteCandidate> = servable.iter().map(|one| route_of(one)).collect();
    if let Err(refusal) = admit(RouteAsk { class }, routes.iter(), rules.policy) {
        return match refusal {
            // The floor or the local-only switch removed every model of the place.
            InferRefusal::Unavailable | InferRefusal::RequiresCloud(_) => {
                Outcome::Nothing(NoPlaceReason::FloorRefused)
            }
            other => Outcome::Other(other.into()),
        };
    }
    let listed: Vec<Listed> = servable.iter().map(|one| one.listed.clone()).collect();
    // A pin is the caller's word for this place: the person's pick for the kind of work does not
    // override it.
    let none = TierMap::default();
    let tiers = if pin.is_some() { &none } else { rules.tiers };
    match choose(need, class, tier, &listed, rules.policy, tiers, rules.auto) {
        Ok(decided) => Outcome::Chosen(Box::new(decided)),
        // The person's pick for this kind of work is a model this place does not have.
        Err(refusal)
            if matches!(
                &refusal.declined,
                Some(porter_infer::Declined {
                    because: porter_infer::DeclinedBecause::NotListed,
                    ..
                })
            ) =>
        {
            Outcome::Nothing(NoPlaceReason::ModelNotOffered)
        }
        Err(refusal) => Outcome::Other(refusal),
    }
}

/// The model to run for `need` inside `allowed`, and the place it is at; or why none may.
pub fn choose_in(
    need: &Need,
    class: DataClass,
    tier: Tier,
    candidates: &[Placed],
    allowed: &Allowed,
    rules: Rules<'_>,
) -> Result<(Decided, PlaceId), Unplaced> {
    let mut reasons: Vec<NoPlaceReason> = Vec::new();
    let mut other: Option<PickRefusal> = None;
    for place in allowed.places() {
        let here: Vec<&Placed> = candidates
            .iter()
            .filter(|one| one.place == *place)
            .collect();
        match attempt(need, class, tier, &here, allowed.pin(place), rules) {
            Outcome::Chosen(decided) => return Ok((*decided, place.clone())),
            Outcome::Nothing(reason) => reasons.push(reason),
            Outcome::Other(refusal) => {
                other.get_or_insert(refusal);
            }
        }
    }
    if let Some(refusal) = other {
        return Err(Unplaced::Other(refusal));
    }
    Err(Unplaced::NoPlace(PlaceRefusal {
        reason: reasons
            .into_iter()
            .min()
            .unwrap_or(NoPlaceReason::NoneCapable),
        would_need: would_need(need, class, tier, candidates, allowed, rules),
    }))
}

/// The kind of place outside the set that would have served the request by the usual rules.
fn would_need(
    need: &Need,
    class: DataClass,
    tier: Tier,
    candidates: &[Placed],
    allowed: &Allowed,
    rules: Rules<'_>,
) -> Option<porter_infer::PlaceKind> {
    let outside: Vec<&Placed> = candidates
        .iter()
        .filter(|one| !allowed.contains(&one.place) && one.serving == Serving::Now)
        .collect();
    let listed: Vec<Listed> = outside.iter().map(|one| one.listed.clone()).collect();
    let decided = choose(
        need,
        class,
        tier,
        &listed,
        rules.policy,
        rules.tiers,
        rules.auto,
    )
    .ok()?;
    outside
        .iter()
        .find(|one| {
            one.listed.card.account == decided.chosen.account
                && one.listed.card.model == decided.chosen.model
        })
        .map(|one| one.place.kind())
}

#[cfg(test)]
mod tests;

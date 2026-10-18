//! The model picker's data (design/31 §5.5): which model the user mapped to each kind and tier,
//! and the plain list a settings page draws. The list ranks nothing: no model is marked best or
//! recommended, and a model the user mapped to a tier is only marked as theirs.

use crate::asker::Chooser;
use crate::error::InferRefusal;
use crate::pick::{AutoMode, Pick};
use crate::readiness::Readiness;
use crate::route::TierChoice;
use crate::slot::Slot;
use porter_core::{AccountId, Billing, Locality, ModelId, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One model on one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModelRef {
    /// The account that serves it.
    pub account: AccountId,
    /// Its id.
    pub model: ModelId,
}

impl ModelRef {
    /// The model written `account/model`, the form a settings row uses; `None` when it is not
    /// two ids joined by one slash.
    pub fn parse(text: &str) -> Option<Self> {
        let (account, model) = text.split_once('/')?;
        Some(Self {
            account: AccountId::parse(account).ok()?,
            model: ModelId::parse(model).ok()?,
        })
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.account, self.model)
    }
}

/// The account a coding agent is chosen under: the text slot's word `agent/<program>` names an
/// outside agent the assistant hands the conversation to, not a model inferd runs. Only the text
/// slot takes it, and Automatic never routes to it.
pub const AGENT_ACCOUNT: &str = "agent";

impl ModelRef {
    /// The choice of the coding agent listed as `program`; `None` when the name is not an id.
    pub fn agent(program: &str) -> Option<Self> {
        Some(Self {
            account: AccountId::parse(AGENT_ACCOUNT).ok()?,
            model: ModelId::parse(program).ok()?,
        })
    }

    /// The agent's program name, when this names a coding agent rather than a model.
    pub fn agent_program(&self) -> Option<&str> {
        (self.account.as_str() == AGENT_ACCOUNT).then(|| self.model.as_str())
    }

    /// Whether this names a coding agent.
    pub fn is_agent(&self) -> bool {
        self.agent_program().is_some()
    }
}

/// The label a settings page draws for a tier. The key segment stays `best` (a name for a slot,
/// kept for compatibility), but the label says what the slot is for and ranks nothing.
pub fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Fast => "Fast",
        Tier::Balanced => "Balanced",
        Tier::Best => "Demanding",
    }
}

/// The user's choice for one kind and tier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TierRow {
    /// The kind.
    pub kind: Slot,
    /// The tier.
    pub tier: Tier,
    /// The model chosen.
    pub model: ModelRef,
}

/// A kind and tier the user set to "auto": settings `ai.model.<kind>.<tier>` = `"auto"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AutoRow {
    /// The kind.
    pub kind: Slot,
    /// The tier.
    pub tier: Tier,
    /// What Automatic does for it.
    pub mode: AutoMode,
}

/// The models Automatic may choose among for a kind, at every tier: settings
/// `ai.auto.pool.<kind>` = a list of `<account>/<model>`. Automatic is a mode the person opts
/// into, and choosing it is also choosing this pool; an empty pool is nothing chosen.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PoolRow {
    /// The kind.
    pub kind: Slot,
    /// The models, in the person's order (the order ranks nothing).
    pub models: Vec<ModelRef>,
    /// Whose pool it is; a file written before helpers had pools reads as the assistant's.
    #[serde(default)]
    pub chooser: Chooser,
}

/// Every choice the user made: settings `ai.model.<kind>.<tier>` = `<account>/<model>` or
/// `"auto"`, and the pool `"auto"` chooses among.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierMap {
    /// The rows naming a model; at most one per kind and tier.
    pub rows: Vec<TierRow>,
    /// The rows set to "auto"; at most one per kind and tier.
    #[serde(default)]
    pub autos: Vec<AutoRow>,
    /// Automatic's pools; at most one per kind.
    #[serde(default)]
    pub pools: Vec<PoolRow>,
}

impl TierMap {
    /// Automatic for `kind` at every tier, choosing among `pool`, and nothing else chosen.
    pub fn automatic(kind: Slot, pool: Vec<ModelRef>) -> Self {
        Self {
            rows: Vec::new(),
            autos: [Tier::Fast, Tier::Balanced, Tier::Best]
                .into_iter()
                .map(|tier| AutoRow {
                    kind,
                    tier,
                    mode: AutoMode::default(),
                })
                .collect(),
            pools: vec![PoolRow {
                kind,
                models: pool,
                chooser: Chooser::Assistant,
            }],
        }
    }

    /// This map with `model` named for `kind` at every tier: the rows and Automatic rows the
    /// map had for `kind` give way, so a session's own model is a named choice like the
    /// person's, never overridden and never replaced by a fallback.
    pub fn naming(&self, kind: Slot, model: ModelRef) -> Self {
        let named = [Tier::Fast, Tier::Balanced, Tier::Best]
            .into_iter()
            .map(|tier| TierRow {
                kind,
                tier,
                model: model.clone(),
            });
        Self {
            rows: self
                .rows
                .iter()
                .filter(|row| row.kind != kind)
                .cloned()
                .chain(named)
                .collect(),
            autos: self
                .autos
                .iter()
                .filter(|row| row.kind != kind)
                .cloned()
                .collect(),
            pools: self.pools.clone(),
        }
    }

    /// The assistant's pool Automatic chooses among for `kind`; empty when the person listed none.
    pub fn pool(&self, kind: Slot) -> &[ModelRef] {
        self.pool_of(&Chooser::Assistant, kind)
    }

    /// The pool `chooser` has for `kind`; empty when it has none. A helper's empty pool is not
    /// filled from the assistant's.
    pub fn pool_of(&self, chooser: &Chooser, kind: Slot) -> &[ModelRef] {
        self.pools
            .iter()
            .find(|row| row.kind == kind && row.chooser == *chooser)
            .map_or(&[], |row| &row.models)
    }

    /// This map as `chooser` reads it: the pools are its own, presented as the pools a pick reads
    /// (so `pick` and the routing built on it need not know who asks). Rows and Automatic rows
    /// are the person's and stay as they are.
    pub fn for_chooser(&self, chooser: &Chooser) -> Self {
        Self {
            rows: self.rows.clone(),
            autos: self.autos.clone(),
            pools: self
                .pools
                .iter()
                .filter(|row| row.chooser == *chooser)
                .map(|row| PoolRow {
                    chooser: Chooser::Assistant,
                    ..row.clone()
                })
                .collect(),
        }
    }

    /// The person's pick for `kind` and `tier`, or `None` when the row is empty: the catalogue's
    /// own choice for most kinds, and nothing chosen for the text slot, which has no default. A
    /// row that names a model wins over an "auto" row for the same slot: a named model is never
    /// overridden, so a file that says both stays with the name. "auto" carries its pool.
    ///
    /// This reads the row and nothing else: it is what a settings page shows. Routing asks
    /// [`TierMap::pick_serving`], which also looks at the person's other tiers.
    pub fn pick(&self, kind: Slot, tier: Tier) -> Option<Pick> {
        // A coding agent is a choice of the text slot only: on any other slot the row is not
        // read, whatever a file says.
        let named = self
            .rows
            .iter()
            .find(|row| row.kind == kind && row.tier == tier)
            .filter(|row| kind == Slot::Text || !row.model.is_agent())
            .map(|row| Pick::Named(row.model.clone()));
        named.or_else(|| {
            self.autos
                .iter()
                .find(|row| row.kind == kind && row.tier == tier)
                .map(|row| Pick::Auto {
                    mode: row.mode,
                    // Automatic chooses among models; it never hands a task to an agent.
                    pool: self
                        .pool(kind)
                        .iter()
                        .filter(|model| !model.is_agent())
                        .cloned()
                        .collect(),
                })
        })
    }

    /// The pick a request at `tier` is served by: the person's pick for that tier, else for
    /// Balanced, else for the remaining tier. Only a pick the person made is used: a slot with
    /// no row at any tier is `None`, and nothing is invented for it.
    pub fn pick_serving(&self, kind: Slot, tier: Tier) -> Option<Pick> {
        let others = match tier {
            Tier::Fast => [Tier::Balanced, Tier::Best],
            Tier::Balanced => [Tier::Fast, Tier::Best],
            Tier::Best => [Tier::Balanced, Tier::Fast],
        };
        self.pick(kind, tier)
            .or_else(|| others.into_iter().find_map(|other| self.pick(kind, other)))
    }

    /// This map for a request that carries tools: Automatic leaves out the models in `failed`
    /// (those that did not show they can make a valid tool call). A named model stays whatever
    /// `failed` says. When the pick serving `kind` at `tier` is Automatic over a pool and every
    /// model of it failed, the answer is [`InferRefusal::NoModelUsesApps`].
    pub fn for_tools(
        &self,
        kind: Slot,
        tier: Tier,
        failed: &BTreeSet<ModelRef>,
    ) -> Result<Self, InferRefusal> {
        if let Some(Pick::Auto { pool, .. }) = self.pick_serving(kind, tier)
            && !pool.is_empty()
            && pool.iter().all(|model| failed.contains(model))
        {
            return Err(InferRefusal::NoModelUsesApps);
        }
        Ok(Self {
            rows: self.rows.clone(),
            autos: self.autos.clone(),
            pools: self
                .pools
                .iter()
                .map(|row| PoolRow {
                    kind: row.kind,
                    chooser: row.chooser.clone(),
                    models: row
                        .models
                        .iter()
                        .filter(|model| !failed.contains(*model))
                        .cloned()
                        .collect(),
                })
                .collect(),
        })
    }
}

/// Whether `model` is the user's choice for `kind` at `tier`. Feeds `route` unchanged.
pub fn tier_choice(map: &TierMap, kind: Slot, tier: Tier, model: &ModelRef) -> TierChoice {
    let chosen = map
        .rows
        .iter()
        .any(|row| row.kind == kind && row.tier == tier && row.model == *model);
    if chosen {
        TierChoice::Chosen
    } else {
        TierChoice::Other
    }
}

/// Whether a model fits in memory now (the supervisor's budget verdict, as a word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// It fits beside what is loaded.
    Fits,
    /// Something loaded must be unloaded first.
    NeedsEviction,
    /// Some layers would run on the CPU.
    NeedsOffload,
    /// It cannot fit on this computer.
    TooLarge,
}

/// How a model may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenceClass {
    /// Open weights, commercial use allowed.
    Open,
    /// Non-commercial weights: listed, never chosen for the user.
    NonCommercial,
    /// A provider's hosted model.
    Proprietary,
}

/// What the picker knows about one model before it draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerInput {
    /// The model.
    pub model: ModelRef,
    /// The name to show.
    pub label: String,
    /// Which kind it serves.
    pub kind: Slot,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Whether it fits in memory.
    pub fit: Fit,
    /// How it may be used.
    pub licence: LicenceClass,
}

/// One row of the plain list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerRow {
    /// The model.
    pub model: ModelRef,
    /// The name to show.
    pub label: String,
    /// Which kind it serves.
    pub kind: Slot,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Whether it fits in memory.
    pub fit: Fit,
    /// How it may be used.
    pub licence: LicenceClass,
    /// The tiers the user mapped it to.
    pub chosen_for: BTreeSet<Tier>,
}

/// The plain list for `kind`: models of that kind only, this computer first, then ready before
/// loadable before downloadable, then the order the cards came in (the catalog's order). No
/// ranking by quality; a non-commercial row is listed and never mapped to a tier by itself.
pub fn picker_rows(kind: Slot, cards: &[PickerInput], map: &TierMap) -> Vec<PickerRow> {
    let mut rows: Vec<PickerRow> = cards
        .iter()
        .filter(|card| card.kind == kind)
        .map(|card| row_of(card, map))
        .collect();
    // `sort_by_key` is stable: equal keys keep the catalog's order.
    rows.sort_by_key(|row| (closeness(&row.locality), soonness(row.readiness)));
    rows
}

fn row_of(card: &PickerInput, map: &TierMap) -> PickerRow {
    let chosen_for = [Tier::Fast, Tier::Balanced, Tier::Best]
        .into_iter()
        .filter(|tier| tier_choice(map, card.kind, *tier, &card.model) == TierChoice::Chosen)
        .collect();
    PickerRow {
        model: card.model.clone(),
        label: card.label.clone(),
        kind: card.kind,
        locality: card.locality.clone(),
        billing: card.billing.clone(),
        readiness: card.readiness,
        fit: card.fit,
        licence: card.licence,
        chosen_for,
    }
}

/// This computer, then the user's other machines, then the cloud; regions do not order.
fn closeness(locality: &Locality) -> u8 {
    match locality {
        Locality::OnDevice => 0,
        Locality::LocalNetwork => 1,
        Locality::Cloud { .. } => 2,
    }
}

/// How soon the model can answer; a download in progress sorts with the downloadable.
fn soonness(readiness: Readiness) -> u8 {
    match readiness {
        Readiness::Ready => 0,
        Readiness::Loading => 1,
        Readiness::Loadable => 2,
        Readiness::Downloading(_) | Readiness::Downloadable => 3,
        Readiness::Unavailable => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(account: &str, model: &str) -> ModelRef {
        ModelRef {
            account: AccountId::parse(account).expect("account"),
            model: ModelId::parse(model).expect("model"),
        }
    }

    #[test]
    fn tier_choice_reads_the_map() {
        let nemotron = model("local", "nemotron");
        let map = TierMap {
            rows: vec![TierRow {
                kind: Slot::VoiceIn,
                tier: Tier::Balanced,
                model: nemotron.clone(),
            }],
            autos: vec![],
            pools: vec![],
        };
        let cases = [
            (
                "the chosen model",
                Slot::VoiceIn,
                Tier::Balanced,
                &nemotron,
                TierChoice::Chosen,
            ),
            (
                "another tier",
                Slot::VoiceIn,
                Tier::Fast,
                &nemotron,
                TierChoice::Other,
            ),
            (
                "another kind",
                Slot::VoiceOut,
                Tier::Balanced,
                &nemotron,
                TierChoice::Other,
            ),
            (
                "another model",
                Slot::VoiceIn,
                Tier::Balanced,
                &model("local", "whisper"),
                TierChoice::Other,
            ),
        ];
        for (name, kind, tier, who, expected) in cases {
            assert_eq!(tier_choice(&map, kind, tier, who), expected, "{name}");
        }
    }

    #[test]
    fn a_coding_agent_is_a_text_choice_and_automatic_never_takes_one() {
        let agent = ModelRef::agent("claude-code").expect("agent");
        assert_eq!(agent.agent_program(), Some("claude-code"));
        assert!(!model("cloud", "claude-haiku").is_agent());
        assert_eq!(ModelRef::agent("Not An Id"), None);
        let named = |kind| TierMap {
            rows: vec![TierRow {
                kind,
                tier: Tier::Balanced,
                model: agent.clone(),
            }],
            autos: vec![],
            pools: vec![],
        };
        let cases = [
            (
                "the text slot takes it",
                Slot::Text,
                Some(Pick::Named(agent.clone())),
            ),
            ("computer use does not", Slot::ComputerUse, None),
            ("neither does voice", Slot::VoiceIn, None),
        ];
        for (name, kind, expected) in cases {
            assert_eq!(named(kind).pick(kind, Tier::Balanced), expected, "{name}");
        }
        let local = model("local", "nemotron");
        let mut pooled = TierMap::automatic(Slot::Text, vec![agent, local.clone()]);
        pooled.rows.clear();
        assert_eq!(
            pooled.pick(Slot::Text, Tier::Fast),
            Some(Pick::Auto {
                mode: AutoMode::default(),
                pool: vec![local],
            })
        );
    }

    fn card(name: &str, kind: Slot, locality: Locality, readiness: Readiness) -> PickerInput {
        PickerInput {
            model: model("local", name),
            label: name.to_owned(),
            kind,
            locality,
            billing: Billing::Free,
            readiness,
            fit: Fit::Fits,
            licence: LicenceClass::Open,
        }
    }

    fn names(rows: &[PickerRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn picker_rows_filter_then_order_by_closeness_then_readiness() {
        use Readiness::*;
        let cloud = |region: Option<&str>| Locality::Cloud {
            region: region.map(|r| porter_core::Region(r.to_owned())),
        };
        let cards = [
            card("far-ready", Slot::Text, cloud(Some("eu-west-1")), Ready),
            card("speech", Slot::VoiceIn, Locality::OnDevice, Ready),
            card("near-down", Slot::Text, Locality::OnDevice, Downloadable),
            card("near-stopped", Slot::Text, Locality::OnDevice, Loadable),
            card("lan", Slot::Text, Locality::LocalNetwork, Ready),
            card("near-ready-a", Slot::Text, Locality::OnDevice, Ready),
            card("near-ready-b", Slot::Text, Locality::OnDevice, Ready),
            card("far-ready-2", Slot::Text, cloud(None), Ready),
            card("near-loading", Slot::Text, Locality::OnDevice, Loading),
            card("near-gone", Slot::Text, Locality::OnDevice, Unavailable),
        ];
        let cases = [
            (
                Slot::Text,
                vec![
                    "near-ready-a",
                    "near-ready-b",
                    "near-loading",
                    "near-stopped",
                    "near-down",
                    "near-gone",
                    "lan",
                    "far-ready",
                    "far-ready-2",
                ],
            ),
            (Slot::VoiceIn, vec!["speech"]),
            (Slot::Rerank, vec![]),
        ];
        for (kind, expected) in cases {
            let rows = picker_rows(kind, &cards, &TierMap::default());
            assert_eq!(names(&rows), expected, "{kind:?}");
        }
    }

    #[test]
    fn picker_rows_mark_the_tiers_the_user_mapped() {
        let a = card("a", Slot::Text, Locality::OnDevice, Readiness::Ready);
        let map = TierMap {
            rows: vec![
                TierRow {
                    kind: Slot::Text,
                    tier: Tier::Fast,
                    model: a.model.clone(),
                },
                TierRow {
                    kind: Slot::Text,
                    tier: Tier::Best,
                    model: a.model.clone(),
                },
                TierRow {
                    kind: Slot::VoiceIn,
                    tier: Tier::Balanced,
                    model: a.model.clone(),
                },
            ],
            autos: vec![],
            pools: vec![],
        };
        let rows = picker_rows(Slot::Text, &[a], &map);
        assert_eq!(rows[0].chosen_for, BTreeSet::from([Tier::Fast, Tier::Best]));
    }

    #[test]
    fn picker_rows_carry_no_ranking_words() {
        let cards = [
            card("a", Slot::Text, Locality::OnDevice, Readiness::Ready),
            card("b", Slot::Text, Locality::LocalNetwork, Readiness::Loadable),
        ];
        let json = serde_json::to_value(picker_rows(Slot::Text, &cards, &TierMap::default()))
            .expect("serializes");
        let mut keys = Vec::new();
        collect_keys(&json, &mut keys);
        for word in [
            "rank",
            "score",
            "recommended",
            "best",
            "top",
            "preferred",
            "default",
        ] {
            assert!(
                !keys.iter().any(|k| k.contains(word)),
                "a row field names `{word}`: {keys:?}"
            );
        }
    }

    fn collect_keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    collect_keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| collect_keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn setting_keys_name_kind_and_tier() {
        assert_eq!(
            Slot::VoiceIn.setting_key(Tier::Fast),
            "ai.model.voice_in.fast"
        );
        assert_eq!(
            Slot::ComputerUse.setting_key(Tier::Balanced),
            "ai.model.computer_use.balanced"
        );
    }

    #[test]
    fn the_best_slot_keeps_its_key_and_shows_another_label() {
        assert_eq!(Slot::Text.setting_key(Tier::Best), "ai.model.text.best");
        let labels: Vec<_> = [Tier::Fast, Tier::Balanced, Tier::Best]
            .into_iter()
            .map(tier_label)
            .collect();
        assert_eq!(labels, ["Fast", "Balanced", "Demanding"]);
        for label in labels {
            for word in ["best", "recommended", "optimal", "smart", "premium"] {
                assert!(!label.to_lowercase().contains(word), "{label}");
            }
        }
    }

    #[test]
    fn the_tier_map_says_named_auto_or_nothing_and_a_name_wins() {
        let a = model("local", "a");
        let map = TierMap {
            rows: vec![
                TierRow {
                    kind: Slot::Text,
                    tier: Tier::Fast,
                    model: a.clone(),
                },
                TierRow {
                    kind: Slot::Text,
                    tier: Tier::Best,
                    model: a.clone(),
                },
            ],
            autos: vec![
                AutoRow {
                    kind: Slot::Text,
                    tier: Tier::Balanced,
                    mode: AutoMode::WarmFirst,
                },
                AutoRow {
                    kind: Slot::Text,
                    tier: Tier::Best,
                    mode: AutoMode::WarmFirst,
                },
            ],
            pools: vec![],
        };
        assert_eq!(
            map.pick(Slot::Text, Tier::Fast),
            Some(Pick::Named(a.clone()))
        );
        assert_eq!(
            map.pick(Slot::Text, Tier::Balanced),
            Some(Pick::Auto {
                mode: AutoMode::WarmFirst,
                pool: Vec::new()
            })
        );
        assert_eq!(map.pick(Slot::Text, Tier::Best), Some(Pick::Named(a)));
        assert_eq!(map.pick(Slot::Embeddings, Tier::Fast), None);
    }

    #[test]
    fn a_tier_with_no_row_is_served_by_the_pick_for_another_tier_and_a_bare_slot_stays_empty() {
        let (a, b) = (model("local", "a"), model("local", "b"));
        let row = |tier, model: &ModelRef| TierRow {
            kind: Slot::Text,
            tier,
            model: model.clone(),
        };
        let only = |tier| TierMap {
            rows: vec![row(tier, &a)],
            ..TierMap::default()
        };
        let both = TierMap {
            rows: vec![row(Tier::Fast, &a), row(Tier::Best, &b)],
            ..TierMap::default()
        };
        let named = |m: &ModelRef| Some(Pick::Named(m.clone()));
        let cases = [
            (
                "balanced only, asked fast",
                only(Tier::Balanced),
                Tier::Fast,
                named(&a),
            ),
            (
                "balanced only, asked best",
                only(Tier::Balanced),
                Tier::Best,
                named(&a),
            ),
            (
                "fast only, asked balanced",
                only(Tier::Fast),
                Tier::Balanced,
                named(&a),
            ),
            (
                "best only, asked fast",
                only(Tier::Best),
                Tier::Fast,
                named(&a),
            ),
            ("the asked tier wins", both.clone(), Tier::Best, named(&b)),
            (
                "balanced asked: fast is looked at first",
                both,
                Tier::Balanced,
                named(&a),
            ),
            ("nothing set refuses", TierMap::default(), Tier::Fast, None),
        ];
        for (name, map, tier, expected) in cases {
            assert_eq!(map.pick_serving(Slot::Text, tier), expected, "{name}");
        }
        let bare = only(Tier::Balanced);
        assert_eq!(
            bare.pick(Slot::Text, Tier::Fast),
            None,
            "the settings page reads the row"
        );
        assert_eq!(
            bare.pick_serving(Slot::VoiceIn, Tier::Fast),
            None,
            "another slot"
        );
    }

    #[test]
    fn for_tools_leaves_failed_models_out_of_automatic_and_never_out_of_a_name() {
        let (a, b, c) = (
            model("local", "a"),
            model("cloud", "b"),
            model("local", "c"),
        );
        let auto = TierMap::automatic(Slot::Text, vec![a.clone(), b.clone(), c.clone()]);
        let failed = |models: &[&ModelRef]| -> BTreeSet<ModelRef> {
            models.iter().map(|m| (*m).clone()).collect()
        };
        let pool = |map: &TierMap| match map.pick(Slot::Text, Tier::Balanced) {
            Some(Pick::Auto { pool, .. }) => pool,
            other => panic!("auto, got {other:?}"),
        };
        assert_eq!(
            pool(
                &auto
                    .for_tools(Slot::Text, Tier::Balanced, &failed(&[&a]))
                    .expect("some pass")
            ),
            [b.clone(), c.clone()]
        );
        assert_eq!(
            pool(
                &auto
                    .for_tools(Slot::Text, Tier::Balanced, &failed(&[]))
                    .expect("all pass")
            ),
            [a.clone(), b.clone(), c.clone()]
        );
        assert_eq!(
            auto.for_tools(Slot::Text, Tier::Fast, &failed(&[&a, &b, &c])),
            Err(InferRefusal::NoModelUsesApps)
        );
        let mut named = TierMap::automatic(Slot::Text, vec![a.clone()]);
        named.rows.push(TierRow {
            kind: Slot::Text,
            tier: Tier::Balanced,
            model: a.clone(),
        });
        let kept = named
            .for_tools(Slot::Text, Tier::Balanced, &failed(&[&a]))
            .expect("a named model is not overridden");
        assert_eq!(kept.pick(Slot::Text, Tier::Balanced), Some(Pick::Named(a)));
        let none = TierMap::default();
        assert_eq!(
            none.for_tools(Slot::Text, Tier::Balanced, &failed(&[&b])),
            Ok(none)
        );
    }

    #[test]
    fn naming_makes_a_sessions_own_model_the_named_choice_at_every_tier() {
        let (a, b) = (model("local", "a"), model("local", "b"));
        let auto = TierMap::automatic(Slot::Text, vec![a.clone()]);
        let named = auto.naming(Slot::Text, b.clone());
        for tier in [Tier::Fast, Tier::Balanced, Tier::Best] {
            assert_eq!(named.pick(Slot::Text, tier), Some(Pick::Named(b.clone())));
        }
        // A row the person set for the slot gives way; other slots are left alone.
        let mut mixed = TierMap::default();
        mixed.rows.push(TierRow {
            kind: Slot::Text,
            tier: Tier::Fast,
            model: a.clone(),
        });
        mixed.rows.push(TierRow {
            kind: Slot::Embeddings,
            tier: Tier::Fast,
            model: a.clone(),
        });
        let named = mixed.naming(Slot::Text, b.clone());
        assert_eq!(named.pick(Slot::Text, Tier::Fast), Some(Pick::Named(b)));
        assert_eq!(
            named.pick(Slot::Embeddings, Tier::Fast),
            Some(Pick::Named(a))
        );
    }

    #[test]
    fn a_tier_map_written_before_auto_rows_still_reads() {
        let old = r#"{"rows":[]}"#;
        let map: TierMap = serde_json::from_str(old).expect("reads");
        assert_eq!(map, TierMap::default());
    }

    #[test]
    fn a_helper_reads_its_own_pool_and_never_the_assistants() {
        let (a, b) = (model("local", "a"), model("cloud", "b"));
        let helper = Chooser::Helper(porter_core::HelperId::parse("h-1").expect("id"));
        let other = Chooser::Helper(porter_core::HelperId::parse("h-2").expect("id"));
        let mut map = TierMap::automatic(Slot::Text, vec![a.clone()]);
        map.pools.push(PoolRow {
            kind: Slot::Text,
            models: vec![b.clone()],
            chooser: helper.clone(),
        });
        assert_eq!(map.pool(Slot::Text), std::slice::from_ref(&a));
        assert_eq!(map.pool_of(&helper, Slot::Text), std::slice::from_ref(&b));
        let pool_in = |view: &TierMap| match view.pick(Slot::Text, Tier::Balanced) {
            Some(Pick::Auto { pool, .. }) => pool,
            other => panic!("auto, got {other:?}"),
        };
        assert_eq!(pool_in(&map.for_chooser(&Chooser::Assistant)), [a]);
        assert_eq!(pool_in(&map.for_chooser(&helper)), [b]);
        assert_eq!(pool_in(&map.for_chooser(&other)), Vec::<ModelRef>::new());
    }

    #[test]
    fn a_pool_row_from_before_helpers_is_the_assistants() {
        let old = r#"{"kind":"text","models":[]}"#;
        let row: PoolRow = serde_json::from_str(old).expect("reads");
        assert_eq!(row.chooser, Chooser::Assistant);
    }
}

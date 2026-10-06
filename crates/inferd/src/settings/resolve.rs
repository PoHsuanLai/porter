//! The file's `[ai]` table (laid over the old `[policy]` and `[tiers]` tables) as [`Settings`].

use super::keys::{from_slug, slug_of};
use super::{
    SPEND_ACCOUNT_DAILY, SPEND_ACCOUNT_MONTHLY, SPEND_APP_DAILY, SPEND_APP_MONTHLY,
    SPEND_CAP_RANGE, SPEND_WARN, SPEND_WARN_DEFAULT, SPEND_WARN_RANGE, ScopeLimits, Settings,
    SpendLimits, SpendLine, cents_to_limit,
};
use crate::config::InferdConfig;
use crate::structured::AiConfig;
use porter_core::{DataClass, Permille, Tier};
use porter_infer::{
    AiKind, AutoRow, ClassFloor, Floor, LocalOnly, ModelRef, Policy, TierMap, TierRow,
};

/// The settings in force, and the paths of the rows whose values were refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// What routing runs under.
    pub settings: Settings,
    /// The rows that held a value they do not accept and fell back to their default (or, for the
    /// policy and the tier map, to the old table's value).
    pub rejected: Vec<String>,
}

/// The model `account/model` names.
pub fn parse_model(text: &str) -> Option<ModelRef> {
    let (account, model) = text.split_once('/')?;
    Some(ModelRef {
        account: porter_core::AccountId::parse(account).ok()?,
        model: porter_core::ModelId::parse(model).ok()?,
    })
}

/// `account/model`: what a model row says for a model.
pub fn model_text(model: &ModelRef) -> String {
    format!("{}/{}", model.account, model.model)
}

fn policy_of(ai: &AiConfig, mut policy: Policy, rejected: &mut Vec<String>) -> Policy {
    if let Some(word) = &ai.local_only {
        match from_slug::<LocalOnly>(word) {
            Some(value) => policy.local_only = value,
            None => rejected.push("ai.local_only".to_owned()),
        }
    }
    for (class, word) in &ai.floor {
        let path = format!("ai.floor.{class}");
        match (from_slug::<DataClass>(class), from_slug::<Floor>(word)) {
            (Some(class), Some(floor)) => {
                match policy.floors.iter_mut().find(|row| row.class == class) {
                    Some(row) => row.floor = floor,
                    None => policy.floors.push(ClassFloor { class, floor }),
                }
            }
            _ => rejected.push(path),
        }
    }
    policy
}

fn tiers_of(ai: &AiConfig, mut tiers: TierMap, rejected: &mut Vec<String>) -> TierMap {
    for (kind_slug, row) in &ai.model {
        for (tier_slug, text) in row {
            let path = format!("ai.model.{kind_slug}.{tier_slug}");
            let (Some(kind), Some(tier)) =
                (from_slug::<AiKind>(kind_slug), from_slug::<Tier>(tier_slug))
            else {
                rejected.push(path);
                continue;
            };
            let slot = |row_kind: AiKind, row_tier: Tier| row_kind == kind && row_tier == tier;
            let named = match text.as_str() {
                "" | "auto" => None,
                other => match parse_model(other) {
                    Some(model) => Some(model),
                    None => {
                        rejected.push(path);
                        continue;
                    }
                },
            };
            // The row at its settings path replaces whatever the old tables said for the slot.
            tiers.rows.retain(|r| !slot(r.kind, r.tier));
            tiers.autos.retain(|r| !slot(r.kind, r.tier));
            match (text.as_str(), named) {
                (_, Some(model)) => tiers.rows.push(TierRow { kind, tier, model }),
                ("auto", None) => tiers.autos.push(AutoRow {
                    kind,
                    tier,
                    mode: Default::default(),
                }),
                _ => {}
            }
        }
    }
    tiers
}

fn spend_of(ai: &AiConfig, rejected: &mut Vec<String>) -> SpendLine {
    let line = |permille: u32| Permille(permille);
    let warn_at = match ai.spend.warn_permille {
        None => line(SPEND_WARN_DEFAULT),
        Some(said) => match u32::try_from(said) {
            Ok(value) if SPEND_WARN_RANGE.contains(&value) => line(value),
            _ => {
                rejected.push(SPEND_WARN.to_owned());
                line(SPEND_WARN_DEFAULT)
            }
        },
    };
    let mut cap = |said: Option<i64>, path: &str| match said {
        None => None,
        Some(said) => match u32::try_from(said) {
            Ok(cents) if SPEND_CAP_RANGE.contains(&cents) => cents_to_limit(cents),
            _ => {
                rejected.push(path.to_owned());
                None
            }
        },
    };
    let limits = SpendLimits {
        account: ScopeLimits {
            daily: cap(ai.spend.account_daily_cents, SPEND_ACCOUNT_DAILY),
            monthly: cap(ai.spend.account_monthly_cents, SPEND_ACCOUNT_MONTHLY),
        },
        app: ScopeLimits {
            daily: cap(ai.spend.app_daily_cents, SPEND_APP_DAILY),
            monthly: cap(ai.spend.app_monthly_cents, SPEND_APP_MONTHLY),
        },
    };
    SpendLine { warn_at, limits }
}

/// The settings a configuration says.
pub fn resolve(config: &InferdConfig) -> Resolved {
    let mut rejected = Vec::new();
    let auto = config.ai.auto.resolve();
    rejected.extend(auto.rejected.iter().map(|path| (*path).to_owned()));
    let settings = Settings {
        policy: policy_of(&config.ai, config.policy(), &mut rejected),
        tiers: tiers_of(&config.ai, config.tiers.clone(), &mut rejected),
        auto: auto.policy,
        spend: spend_of(&config.ai, &mut rejected),
    };
    Resolved { settings, rejected }
}

/// What the file says for one slot, as the picker's value: `""`, `auto` or `account/model`.
pub fn slot_value(tiers: &TierMap, kind: AiKind, tier: Tier) -> String {
    match tiers.pick(kind, tier) {
        None => String::new(),
        Some(porter_infer::Pick::Named(model)) => model_text(&model),
        Some(porter_infer::Pick::Auto(_)) => "auto".to_owned(),
    }
}

/// A floor as the row's value.
pub fn floor_value(policy: &Policy, class: DataClass) -> String {
    slug_of(&policy.floor(class))
}

//! The settings inferd serves and reads (design/22 section 5, the Intelligence page): the rows of
//! `dist/inferd.settings.toml` and the `ai.model.<kind>.<tier>` picker, a live module.
//!
//! * [`Settings`] is what routing runs under: the policy (`ai.local_only`, `ai.floor.<class>`), the
//!   tier map (`ai.model.<kind>.<tier>`), Automatic (`ai.auto.*`) and the spend line
//!   (`ai.spend.warn_permille`). [`resolve`] reads it from the file's `[ai]` table, laid over the
//!   old `[policy]` and `[tiers]` tables; a value a row does not accept falls back to the row's
//!   default, for that field only, and is named in [`Resolved::rejected`].
//! * [`Live`] holds the settings in force, shared by every clone of `Engines`; a session is routed
//!   by what is in force when it opens.
//! * [`ConfigFile`] reads and edits the file, and [`Reload`] puts what it says in force again: the
//!   daemon polls the file's mtime, `Rescan` reloads at once, and a `Set` of the module applies
//!   what it wrote.
//! * [`InferdSettings`] is `org.quire.SettingsModule1` at `INFERENCE_SETTINGS_PATH`.

mod file;
mod keys;
mod module;
mod resolve;

pub use file::{ConfigFile, FileError, Reload, ReloadFailed, Reloaded};
pub use keys::{CLASSES, KINDS, TIERS, from_slug, model_path, slug_of};
pub use module::{InferdSettings, serve_settings};
pub use resolve::{Resolved, floor_value, model_text, parse_model, resolve, slot_value};

use porter_core::{AccountId, AppId, MicroUsd, Permille};
use porter_infer::{AutoPolicy, Period, Policy, SpendCap, SpendScope, TierMap};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, RwLock};

/// `ai.spend.warn_permille`.
pub const SPEND_WARN: &str = "ai.spend.warn_permille";
/// What the row says without a file, in thousandths of a cap.
pub const SPEND_WARN_DEFAULT: u32 = 800;
/// The values the row accepts.
pub const SPEND_WARN_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;

/// The four cap rows, in whole US cents; `0` is no cap. A cap on an account counts everything
/// spent through it, whoever spent it; a cap on an app counts everything it spends, whichever
/// account paid.
pub const SPEND_ACCOUNT_DAILY: &str = "ai.spend.account_daily_cents";
/// `ai.spend.account_monthly_cents`.
pub const SPEND_ACCOUNT_MONTHLY: &str = "ai.spend.account_monthly_cents";
/// `ai.spend.app_daily_cents`.
pub const SPEND_APP_DAILY: &str = "ai.spend.app_daily_cents";
/// `ai.spend.app_monthly_cents`.
pub const SPEND_APP_MONTHLY: &str = "ai.spend.app_monthly_cents";
/// The values the cap rows accept: up to a hundred thousand dollars.
pub const SPEND_CAP_RANGE: std::ops::RangeInclusive<u32> = 0..=10_000_000;
/// Micro-dollars in a cent.
const MICRO_USD_PER_CENT: u64 = 10_000;

/// The `[ai.spend]` table, as written. Signed, so a negative number reaches the range check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendConfig {
    /// `ai.spend.warn_permille`.
    #[serde(default)]
    pub warn_permille: Option<i64>,
    /// `ai.spend.account_daily_cents`.
    #[serde(default)]
    pub account_daily_cents: Option<i64>,
    /// `ai.spend.account_monthly_cents`.
    #[serde(default)]
    pub account_monthly_cents: Option<i64>,
    /// `ai.spend.app_daily_cents`.
    #[serde(default)]
    pub app_daily_cents: Option<i64>,
    /// `ai.spend.app_monthly_cents`.
    #[serde(default)]
    pub app_monthly_cents: Option<i64>,
}

/// One scope's limits: none where the person set none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeLimits {
    /// Per calendar day.
    pub daily: Option<MicroUsd>,
    /// Per calendar month.
    pub monthly: Option<MicroUsd>,
}

/// The caps the person set: on each account, and on each app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpendLimits {
    /// Per account.
    pub account: ScopeLimits,
    /// Per app.
    pub app: ScopeLimits,
}

/// The share of a cap at which a request is warned about, and the caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendLine {
    /// `ai.spend.warn_permille`.
    pub warn_at: Permille,
    /// `ai.spend.{account,app}_{daily,monthly}_cents`.
    pub limits: SpendLimits,
}

impl Default for SpendLine {
    fn default() -> Self {
        Self {
            warn_at: Permille(SPEND_WARN_DEFAULT),
            limits: SpendLimits::default(),
        }
    }
}

/// A cap row's value in micro-dollars; `None` for no cap (`0`).
pub fn cents_to_limit(cents: u32) -> Option<MicroUsd> {
    (cents > 0).then(|| MicroUsd(u64::from(cents) * MICRO_USD_PER_CENT))
}

impl SpendLine {
    /// A cap on `scope` over `period` with this line as its warning: the one place inferd makes a
    /// [`SpendCap`], so `SpendCap.warn_at` is always the row's value.
    pub fn cap(self, scope: SpendScope, period: Period, limit: MicroUsd) -> SpendCap {
        SpendCap {
            scope,
            period,
            limit,
            warn_at: self.warn_at,
        }
    }

    /// The caps that bear on one more request by `app` through `account`.
    pub fn caps_for(self, app: &AppId, account: &AccountId) -> Vec<SpendCap> {
        let SpendLimits {
            account: on_account,
            app: on_app,
        } = self.limits;
        let rows = [
            (
                SpendScope::Account(account.clone()),
                Period::Daily,
                on_account.daily,
            ),
            (
                SpendScope::Account(account.clone()),
                Period::Monthly,
                on_account.monthly,
            ),
            (SpendScope::App(app.clone()), Period::Daily, on_app.daily),
            (
                SpendScope::App(app.clone()),
                Period::Monthly,
                on_app.monthly,
            ),
        ];
        rows.into_iter()
            .filter_map(|(scope, period, limit)| Some(self.cap(scope, period, limit?)))
            .collect()
    }
}

/// What routing runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `ai.local_only` and `ai.floor.<class>`.
    pub policy: Policy,
    /// `ai.model.<kind>.<tier>`.
    pub tiers: TierMap,
    /// `ai.auto.*`.
    pub auto: AutoPolicy,
    /// `ai.spend.*`.
    pub spend: SpendLine,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            policy: Policy::proposed(),
            tiers: TierMap::default(),
            auto: AutoPolicy::default(),
            spend: SpendLine::default(),
        }
    }
}

/// The settings in force: replaced whole, read as a snapshot.
#[derive(Debug, Clone, Default)]
pub struct Live(Arc<RwLock<Arc<Settings>>>);

impl Live {
    /// A holder of `settings`.
    pub fn new(settings: Settings) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(settings))))
    }

    /// What is in force.
    pub fn get(&self) -> Arc<Settings> {
        Arc::clone(&self.0.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Puts `settings` in force.
    pub fn set(&self, settings: Settings) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(settings);
    }
}

#[cfg(test)]
mod tests;

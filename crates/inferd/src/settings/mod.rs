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
pub use keys::{CLASSES, KINDS, TIERS, model_path, slug_of};
pub use module::{InferdSettings, serve_settings};
pub use resolve::{Resolved, floor_value, model_text, parse_model, resolve, slot_value};

use porter_core::MicroUsd;
use porter_core::Permille;
use porter_infer::{AutoPolicy, Period, Policy, SpendCap, SpendScope, TierMap};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, RwLock};

/// `ai.spend.warn_permille`.
pub const SPEND_WARN: &str = "ai.spend.warn_permille";
/// What the row says without a file, in thousandths of a cap.
pub const SPEND_WARN_DEFAULT: u32 = 800;
/// The values the row accepts.
pub const SPEND_WARN_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;

/// The `[ai.spend]` table, as written. Signed, so a negative number reaches the range check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendConfig {
    /// `ai.spend.warn_permille`.
    #[serde(default)]
    pub warn_permille: Option<i64>,
}

/// The share of a cap at which a request is warned about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendLine {
    /// `ai.spend.warn_permille`.
    pub warn_at: Permille,
}

impl Default for SpendLine {
    fn default() -> Self {
        Self {
            warn_at: Permille(SPEND_WARN_DEFAULT),
        }
    }
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

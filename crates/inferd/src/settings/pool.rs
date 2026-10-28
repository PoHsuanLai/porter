//! Writing an Automatic pool: the one check of its models and the one write of its row, for the
//! assistant's pool (a `Set` of the settings module) and for a helper's (`SetHelperPool`, which
//! Settings and the shell may call). Nothing is chosen for the person: a pool holds the models
//! they picked, and an empty helper pool is no row, so that helper is not on Automatic.

use super::file::Reload;
use super::keys::{pool_path, pool_row_key};
use super::resolve::{model_text, parse_model};
use crate::cloud::picker::{Choice, choices};
use ds_settings::live::LiveError;
use porter_capability::Locality;
use porter_core::HelperId;
use porter_core::consent::Usage;
use porter_core::{AppId, AppName, DataClass, Isolation};
use porter_infer::{Chooser, ModelRef, Slot};
use porter_router::router::slots_of;

/// The `[ai.auto.pool]` table of `inferd.toml`, where a helper's row lives.
const POOL_TABLE: &str = "ai.auto.pool";

/// The app the picker asks accountd about the person's accounts as: `Verdicts` lists every account
/// that serves a language need, and the picker reads only that an account exists.
fn picker_app() -> Option<AppId> {
    Some(AppId {
        name: AppName::parse("org.quire.Settings").ok()?,
        isolation: Isolation::Unsandboxed,
    })
}

/// The models inferd knows for `kind`, as `account/model`, each with where it runs.
pub(super) fn known_for(reload: &Reload, kind: Slot) -> Vec<(String, Locality)> {
    reload
        .engines()
        .listed()
        .into_iter()
        .filter(|one| slots_of(&one.card).contains(&kind))
        .map(|one| {
            let text = model_text(&ModelRef {
                account: one.card.account,
                model: one.card.model,
            });
            (text, one.card.locality)
        })
        .collect()
}

/// The hosted models that fit `slot`, company then model, with whether the person has an account
/// that reaches each; none when this daemon serves none.
pub(super) async fn hosted(reload: &Reload, slot: Slot) -> Vec<Choice> {
    let (Some(cloud), Some(app)) = (reload.engines().cloud().cloned(), picker_app()) else {
        return Vec::new();
    };
    let accounts = cloud
        .accounts(&app, DataClass::Prompt, Usage::Interactive)
        .await;
    let mut found = choices(cloud.entries(), &accounts, slot);
    // The servers of the person's own list chat models only, after the companies'.
    if matches!(slot, Slot::Text) {
        found.extend(cloud.own_choices(&accounts).await);
    }
    found
}

/// Whether `text` names a model the person may choose for `kind`.
pub(super) fn choosable(reload: &Reload, kind: Slot, text: &str, hosted: &[Choice]) -> bool {
    parse_model(text).is_some()
        && (known_for(reload, kind)
            .iter()
            .any(|(known, _)| known == text)
            || hosted.iter().any(|choice| choice.value == text))
}

/// The models a pool value names, each one inferd knows for `kind`, without repeats.
pub(super) async fn validated(
    reload: &Reload,
    kind: Slot,
    value: toml::Value,
) -> Result<Vec<String>, LiveError> {
    let toml::Value::Array(items) = value else {
        return Err(LiveError::BadValue("a pool is a list of models".into()));
    };
    let hosted = hosted(reload, kind).await;
    let mut texts = Vec::with_capacity(items.len());
    for item in items {
        let toml::Value::String(text) = item else {
            return Err(LiveError::BadValue("a model is text".into()));
        };
        if !choosable(reload, kind, &text, &hosted) {
            return Err(LiveError::BadValue(format!(
                "{text}: not a model inferd knows for {}",
                kind.slug()
            )));
        }
        if !texts.contains(&text) {
            texts.push(text);
        }
    }
    Ok(texts)
}

/// Writes `chooser`'s pool for `kind` and puts it in force. The assistant's pool is a row even
/// when empty; a helper's empty pool removes its row.
pub(super) async fn write(
    reload: &Reload,
    kind: Slot,
    chooser: &Chooser,
    texts: Vec<String>,
) -> Result<(), LiveError> {
    let empty = texts.is_empty();
    let list = toml::Value::Array(texts.into_iter().map(toml::Value::String).collect());
    let _one_writer = reload.writing().lock().await;
    match chooser {
        Chooser::Assistant => reload.file().set(&pool_path(kind), list),
        Chooser::Helper(_) => reload.file().set_key(
            POOL_TABLE,
            &pool_row_key(kind, chooser),
            (!empty).then_some(list),
        ),
    }
    .map_err(failed)?;
    reload.now().map_err(failed).map(|_| ())
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

/// The writer of helpers' Automatic pools over the daemon's file.
#[derive(Debug, Clone)]
pub struct HelperPools {
    reload: Reload,
}

impl HelperPools {
    /// The writer over `reload`'s file and engines.
    pub fn new(reload: Reload) -> Self {
        Self { reload }
    }

    /// Sets the models Automatic chooses among for `helper` in `slot` and puts them in force. Each
    /// must be a model inferd knows for the slot; none are added for the person. An empty list
    /// takes the helper off Automatic for the slot.
    pub async fn set(
        &self,
        slot: Slot,
        helper: &HelperId,
        models: Vec<String>,
    ) -> Result<(), LiveError> {
        let value = toml::Value::Array(models.into_iter().map(toml::Value::String).collect());
        let texts = validated(&self.reload, slot, value).await?;
        write(&self.reload, slot, &Chooser::Helper(helper.clone()), texts).await
    }
}

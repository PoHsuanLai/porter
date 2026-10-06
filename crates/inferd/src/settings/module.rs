//! `org.quire.SettingsModule1` at `/org/quire/Inference1/settings` (design/22 section 9.4): the
//! `ai.model.<slot>.<tier>` picker, whose choices are the models inferd knows now (and, per slot,
//! the curated hosted models, company then model, each marked when no account reaches it). Only the
//! `Settings` role may `Describe`, `Get` or `Set`. A `Set` validates the model, writes it to
//! `inferd.toml` at that path and puts it in force; the next session is routed by it.

use super::file::Reload;
use super::keys::{SLOTS, TIERS, model_path, parse_model_path, slots_of};
use super::resolve::{model_text, parse_model, slot_value};
use crate::cloud::picker::{Availability, Choice, NEEDS_ACCOUNT, choices};
use crate::peers::{Peers, Role};
use ds_settings::live::{Access, Caller, LiveError, LiveModule, LiveSchema, Verdict, serve};
use ds_settings::schema::{
    AgentSetting, ChoiceWord, Exposure, Help, KeyKind, KeyPath, KeySpec, Label, Page, Section,
    UnavailableReason, WordLabels,
};
use porter_core::consent::Usage;
use porter_core::{AppId, AppName, DataClass, Isolation, Tier};
use porter_dbus::INFERENCE_SETTINGS_PATH;
use porter_infer::{Slot, tier_label};

/// The picker value that is Automatic.
const AUTO: &str = "auto";

/// The app the picker asks accountd about the person's accounts as: `Verdicts` lists every account
/// that serves a language need, and the picker reads only that an account exists.
fn picker_app() -> Option<AppId> {
    Some(AppId {
        name: AppName::parse("org.quire.Settings").ok()?,
        isolation: Isolation::Unsandboxed,
    })
}

/// The module over the daemon's engines and its file.
pub struct InferdSettings<P> {
    peers: P,
    reload: Reload,
    writing: tokio::sync::Mutex<()>,
}

impl<P> std::fmt::Debug for InferdSettings<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InferdSettings")
            .field("file", &self.reload.file().path())
            .finish_non_exhaustive()
    }
}

impl<P: Peers> InferdSettings<P> {
    /// A module that decides who may use it through `peers` and writes through `reload`'s file.
    pub fn new(peers: P, reload: Reload) -> Self {
        Self {
            peers,
            reload,
            writing: tokio::sync::Mutex::new(()),
        }
    }

    /// The models inferd knows for `kind`, as `account/model`, with their names.
    fn models_for(&self, kind: Slot) -> Vec<String> {
        self.reload
            .engines()
            .listed()
            .into_iter()
            .filter(|one| slots_of(&one.card).contains(&kind))
            .map(|one| {
                model_text(&porter_infer::ModelRef {
                    account: one.card.account,
                    model: one.card.model,
                })
            })
            .collect()
    }

    /// The hosted models that fit `slot`, company then model, with whether the person has an
    /// account that reaches each; none when this daemon serves none.
    async fn hosted(&self, slot: Slot) -> Vec<Choice> {
        let (Some(cloud), Some(app)) = (self.reload.engines().cloud().cloned(), picker_app())
        else {
            return Vec::new();
        };
        choices(
            cloud.entries(),
            &cloud
                .accounts(&app, DataClass::Prompt, Usage::Interactive)
                .await,
            slot,
        )
    }

    /// The path of the old-kind row for this slot and tier, when the file has one.
    fn legacy_row(&self, slot: Slot, tier: Tier) -> Option<String> {
        let old = slot.legacy_slug()?;
        let path = model_path(slot, tier).replacen(slot.slug(), old, 1);
        let config = self.reload.file().read().ok()?;
        let (_, tier_slug) = path.strip_prefix("ai.model.")?.split_once('.')?;
        config
            .ai
            .model
            .get(old)
            .is_some_and(|row| row.contains_key(tier_slug))
            .then_some(path)
    }

    fn current(&self, kind: Slot, tier: Tier) -> String {
        slot_value(&self.reload.engines().settings().tiers, kind, tier)
    }

    fn spec(&self, kind: Slot, tier: Tier, hosted: &[Choice]) -> KeySpec {
        let current = self.current(kind, tier);
        let mut variants = vec![String::new(), AUTO.to_owned()];
        variants.extend(self.models_for(kind));
        variants.extend(hosted.iter().map(|choice| choice.value.clone()));
        if !variants.contains(&current) {
            variants.push(current);
        }
        let mut labels = WordLabels::default();
        labels
            .0
            .insert(String::new(), "Catalogue default".to_owned());
        labels.0.insert(AUTO.to_owned(), "Automatic".to_owned());
        for model in variants.iter().skip(2) {
            let name = model
                .split_once('/')
                .map_or(model.as_str(), |(_, name)| name);
            labels.0.insert(model.clone(), name.to_owned());
        }
        for choice in hosted {
            labels.0.insert(choice.value.clone(), choice.label.clone());
        }
        // A hosted model no account reaches is listed, greyed, with the reason: the skeleton
        // refuses a Set of it (`Unavailable`), so a pick names only what can run.
        let unavailable = hosted
            .iter()
            .filter(|choice| choice.available == Availability::NeedsAccount)
            .map(|choice| {
                (
                    ChoiceWord(choice.value.clone()),
                    UnavailableReason(NEEDS_ACCOUNT.to_owned()),
                )
            })
            .collect();
        KeySpec {
            path: KeyPath(model_path(kind, tier)),
            kind: KeyKind::Menu { variants },
            default: toml::Value::String(String::new()),
            // Grouped by job: a section per slot, the tiers listed under it.
            label: Label(tier_label(tier).to_owned()),
            help: Help(
                "The model used for this slot at this tier. Automatic picks one that \
                 is allowed and already loaded when it can."
                    .to_owned(),
            ),
            page: Page::Intelligence,
            section: Section(format!("Models / {}", slot_label(kind))),
            exposure: Exposure::Basic,
            labels,
            unavailable,
            agent: AgentSetting::HandsOff,
        }
    }
}

fn slot_label(slot: Slot) -> &'static str {
    match slot {
        Slot::Text => "Text",
        Slot::VoiceIn => "Speech to text",
        Slot::VoiceOut => "Text to speech",
        Slot::ImageIn => "Reading images",
        Slot::ComputerUse => "Computer use",
        Slot::Embeddings => "Embeddings",
        Slot::ImageGen => "Image generation",
        Slot::Rerank => "Reranking",
    }
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

impl<P: Peers> LiveModule for InferdSettings<P> {
    async fn permit(&self, caller: &Caller, _access: Access) -> Verdict {
        match self.peers.caller_of(&caller.sender.0).await {
            Some(who) if who.role == Role::Settings => Verdict::Allow,
            _ => Verdict::Refuse,
        }
    }

    async fn describe(&self) -> LiveSchema {
        let mut key = Vec::new();
        for kind in SLOTS {
            let hosted = self.hosted(kind).await;
            let known = !self.models_for(kind).is_empty()
                || !hosted.is_empty()
                || TIERS
                    .iter()
                    .any(|tier| !self.current(kind, *tier).is_empty());
            if known {
                key.extend(TIERS.map(|tier| self.spec(kind, tier, &hosted)));
            }
        }
        LiveSchema { version: 1, key }
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        let (kind, tier) =
            parse_model_path(&key.0).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        Ok(toml::Value::String(self.current(kind, tier)))
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        let (slot, tier) =
            parse_model_path(&key.0).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        let kind = slot;
        let toml::Value::String(text) = value else {
            return Err(LiveError::BadValue("a model is text".into()));
        };
        let hosted = self.hosted(kind).await;
        let known = text.is_empty()
            || text == AUTO
            || parse_model(&text).is_some_and(|_| {
                self.models_for(kind).contains(&text)
                    || hosted.iter().any(|choice| choice.value == text)
            });
        if !known {
            return Err(LiveError::BadValue(format!(
                "{text}: not a model inferd knows for {}",
                kind.slug()
            )));
        }
        let _one_writer = self.writing.lock().await;
        // Always the slot's own path; an old kind row for the same slot, if the file still has
        // one, is kept in step so it cannot say otherwise.
        let file = self.reload.file();
        let value = toml::Value::String(text);
        file.set(&model_path(slot, tier), value.clone())
            .map_err(failed)?;
        if let Some(old) = self.legacy_row(slot, tier) {
            file.set(&old, value).map_err(failed)?;
        }
        self.reload.now().map_err(failed).map(|_| ())
    }
}

/// Serves the module at [`INFERENCE_SETTINGS_PATH`].
pub async fn serve_settings<P: Peers>(
    connection: &zbus::Connection,
    module: InferdSettings<P>,
) -> zbus::Result<()> {
    serve(connection, INFERENCE_SETTINGS_PATH, module)
        .await
        .map(|_| ())
        .map_err(|e| zbus::Error::Failure(e.to_string()))
}

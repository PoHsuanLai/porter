//! `org.quire.SettingsModule1` at `/org/quire/Inference1/settings` (design/22 section 9.4): the
//! `ai.model.<slot>.<tier>` picker, whose choices are the models inferd knows now (and, per slot,
//! the curated hosted models, company then model, each marked when no account reaches it). Only the
//! `Settings` role may `Describe`, `Get` or `Set`. A `Set` validates the model, writes it to
//! `inferd.toml` at that path and puts it in force; the next session is routed by it.

use super::file::Reload;
use super::keys::{
    SLOTS, TIERS, model_path, parse_model_path, parse_pool_path, pool_path, slots_of,
};
use super::missing::{Engine, MissingEngines};
use super::resolve::{model_text, parse_model, slot_value};
use crate::cloud::picker::{Availability, Choice, NEEDS_ACCOUNT, choices};
use crate::keys::KeyRows;
use crate::peers::{Peers, Role};
use ds_settings::live::{Access, Caller, LiveError, LiveModule, LiveSchema, Verdict, serve};
use ds_settings::schema::{
    AgentSetting, ChoiceGroup, ChoicePlace, ChoiceWord, CompanyName, EngineName, Exposure, Help,
    HostName, KeyKind, KeyPath, KeySpec, Label, Note, Page, Section, UnavailableReason, WordLabels,
};
use porter_core::consent::Usage;
use porter_router::local::LocalModel;
use std::collections::BTreeMap;
use porter_core::{AppId, AppName, DataClass, Isolation, Locality, Tier};
use porter_dbus::INFERENCE_SETTINGS_PATH;
use porter_infer::{ModelRef, Slot, tier_label};

/// The picker value that is Automatic.
const AUTO: &str = "auto";

/// The group the models this computer runs are listed under, before each company's hosted ones.
const ON_THIS_COMPUTER: &str = "On this computer";

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
    /// The `accounts.<id>.*` rows of the AI accounts, when this daemon hosts them.
    keys: Option<std::sync::Arc<dyn KeyRows>>,
    /// The engine programs this computer lacks, named in a slot with no model here.
    missing: MissingEngines,
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
            keys: None,
            missing: MissingEngines::default(),
        }
    }

    /// The same module, saying in a slot with no model on this computer which missing program
    /// would run one.
    #[must_use]
    pub fn with_missing(self, missing: MissingEngines) -> Self {
        Self { missing, ..self }
    }

    /// The same module, also listing and changing the AI accounts: their rows, the permits'
    /// Revoke rows included (`accounts.<id>.*`, the same words accountd's rows had).
    #[must_use]
    pub fn with_keys(self, keys: std::sync::Arc<dyn KeyRows>) -> Self {
        Self {
            keys: Some(keys),
            ..self
        }
    }

    /// The models inferd knows for `kind`, as `account/model`, with their names.
    fn models_for(&self, kind: Slot) -> Vec<String> {
        self.known_for(kind)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    /// `models_for`, each with where it runs.
    fn known_for(&self, kind: Slot) -> Vec<(String, Locality)> {
        self.reload
            .engines()
            .listed()
            .into_iter()
            .filter(|one| slots_of(&one.card).contains(&kind))
            .map(|one| {
                let text = model_text(&porter_infer::ModelRef {
                    account: one.card.account,
                    model: one.card.model,
                });
                (text, one.card.locality)
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

    /// The picker's help.
    fn help() -> Help {
        Help(
            "The model used for this slot at this tier. Automatic picks one of the models you \
             list for it, one already loaded when it can."
                .to_owned(),
        )
    }

    /// Why nothing runs on this computer, when no model of the slot does and a program is
    /// missing.
    fn note(&self, kind: Slot) -> Option<Note> {
        let here = self
            .known_for(kind)
            .iter()
            .any(|(_, locality)| *locality == Locality::OnDevice);
        self.missing.note(kind).filter(|_| !here).map(Note)
    }

    /// Where each listed model of `kind` runs: the engine of one on this computer, the computer
    /// of one on another of the person's. A model with no place to name has no entry.
    fn local_places(&self, kind: Slot) -> BTreeMap<ChoiceWord, ChoicePlace> {
        let engines = self.reload.engines();
        engines
            .listed()
            .into_iter()
            .filter(|one| slots_of(&one.card).contains(&kind))
            .filter_map(|one| {
                let model = ModelRef {
                    account: one.card.account,
                    model: one.card.model,
                };
                let found = engines.local_model(&model);
                let place = match one.card.locality {
                    Locality::OnDevice => ChoicePlace::ThisComputer {
                        engine: found.as_deref().and_then(engine_of),
                    },
                    Locality::LocalNetwork => ChoicePlace::YourComputer {
                        host: HostName(
                            found?
                                .attached
                                .as_ref()?
                                .computer
                                .as_ref()?
                                .as_str()
                                .to_owned(),
                        ),
                    },
                    _ => return None,
                };
                Some((ChoiceWord(model_text(&model)), place))
            })
            .collect()
    }

    /// The models the person may choose for `kind`, with their names, groups and the reason
    /// each greyed one cannot run; `keep` are values the file holds that are listed even when
    /// inferd no longer knows them.
    fn model_choices(&self, kind: Slot, hosted: &[Choice], keep: &[String]) -> ModelChoices {
        let known = self.known_for(kind);
        let mut variants: Vec<String> = known.iter().map(|(text, _)| text.clone()).collect();
        variants.extend(hosted.iter().map(|choice| choice.value.clone()));
        for held in keep {
            if !variants.contains(held) {
                variants.push(held.clone());
            }
        }
        let mut labels = WordLabels::default();
        for model in &variants {
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
        // The models this computer runs under one group (an engine model whose card says Cloud is
        // not one of them: it stays ungrouped), hosted ones under their company.
        let groups = known
            .into_iter()
            .filter(|(_, locality)| *locality == Locality::OnDevice)
            .map(|(word, _)| (word, ON_THIS_COMPUTER.to_owned()))
            .chain(
                hosted
                    .iter()
                    .map(|choice| (choice.value.clone(), choice.company.clone())),
            )
            .map(|(word, group)| (ChoiceWord(word), ChoiceGroup(group)))
            .collect();
        let places = self
            .local_places(kind)
            .into_iter()
            .chain(hosted.iter().map(|choice| {
                let company = CompanyName(choice.company.clone());
                (
                    ChoiceWord(choice.value.clone()),
                    ChoicePlace::Cloud { company },
                )
            }))
            .collect();
        ModelChoices {
            variants,
            labels,
            unavailable,
            groups,
            places,
        }
    }

    fn spec(&self, kind: Slot, tier: Tier, hosted: &[Choice]) -> KeySpec {
        let current = self.current(kind, tier);
        let keep: Vec<String> = [current]
            .into_iter()
            .filter(|held| !held.is_empty() && held != AUTO)
            .collect();
        let ModelChoices {
            variants: models,
            mut labels,
            unavailable,
            groups,
            places,
        } = self.model_choices(kind, hosted, &keep);
        // The text slot has no default: unset, the assistant asks the person to choose
        // (owner, 2026-10-16). The other slots still take the catalogue's choice. The unset
        // choice and Automatic stay ungrouped, so they come first.
        let unset = match kind {
            Slot::Text => "Not chosen",
            _ => "Catalogue default",
        };
        labels.0.insert(String::new(), unset.to_owned());
        labels.0.insert(AUTO.to_owned(), "Automatic".to_owned());
        let variants = [String::new(), AUTO.to_owned()]
            .into_iter()
            .chain(models)
            .collect();
        KeySpec {
            path: KeyPath(model_path(kind, tier)),
            kind: KeyKind::Menu { variants },
            default: toml::Value::String(String::new()),
            // Grouped by job: a section per slot, the tiers listed under it.
            label: Label(tier_label(tier).to_owned()),
            help: Self::help(),
            page: Page::Intelligence,
            section: Section(format!("Models / {}", slot_label(kind))),
            exposure: Exposure::Basic,
            labels,
            unavailable,
            groups,
            places,
            note: self.note(kind),
            agent: AgentSetting::HandsOff,
        }
    }

    /// The models in `kind`'s Automatic pool now, as `account/model`.
    fn current_pool(&self, kind: Slot) -> Vec<String> {
        self.reload
            .engines()
            .settings()
            .tiers
            .pool(kind)
            .iter()
            .map(model_text)
            .collect()
    }

    /// `ai.auto.pool.<slot>`: the list Automatic chooses among, the same choices as the slot's
    /// picker without the unset one and Automatic itself.
    fn pool_spec(&self, kind: Slot, hosted: &[Choice]) -> KeySpec {
        let ModelChoices {
            variants,
            labels,
            unavailable,
            groups,
            places,
        } = self.model_choices(kind, hosted, &self.current_pool(kind));
        KeySpec {
            path: KeyPath(pool_path(kind)),
            kind: KeyKind::List(Box::new(KeyKind::Menu { variants })),
            default: toml::Value::Array(Vec::new()),
            label: Label("Automatic chooses from".to_owned()),
            help: Help(
                "The models Automatic may use for this slot, at every tier. With none \
                 listed, Automatic has nothing to choose from and asks you to pick."
                    .to_owned(),
            ),
            page: Page::Intelligence,
            section: Section(format!("Models / {}", slot_label(kind))),
            exposure: Exposure::Basic,
            labels,
            unavailable,
            groups,
            places,
            note: self.note(kind),
            agent: AgentSetting::HandsOff,
        }
    }

    /// Whether `text` names a model the person may choose for `kind`.
    fn choosable(&self, kind: Slot, text: &str, hosted: &[Choice]) -> bool {
        parse_model(text).is_some()
            && (self.models_for(kind).iter().any(|known| known == text)
                || hosted.iter().any(|choice| choice.value == text))
    }

    async fn set_pool(&self, kind: Slot, value: toml::Value) -> Result<(), LiveError> {
        let toml::Value::Array(items) = value else {
            return Err(LiveError::BadValue("a pool is a list of models".into()));
        };
        let hosted = self.hosted(kind).await;
        let mut texts = Vec::with_capacity(items.len());
        for item in items {
            let toml::Value::String(text) = item else {
                return Err(LiveError::BadValue("a model is text".into()));
            };
            if !self.choosable(kind, &text, &hosted) {
                return Err(LiveError::BadValue(format!(
                    "{text}: not a model inferd knows for {}",
                    kind.slug()
                )));
            }
            if !texts.contains(&text) {
                texts.push(text);
            }
        }
        let _one_writer = self.writing.lock().await;
        let list = texts.into_iter().map(toml::Value::String).collect();
        self.reload
            .file()
            .set(&pool_path(kind), toml::Value::Array(list))
            .map_err(failed)?;
        self.reload.now().map_err(failed).map(|_| ())
    }
}

/// What a model menu offers: the values, their names, the greyed ones with why, and groups.
struct ModelChoices {
    variants: Vec<String>,
    labels: WordLabels,
    unavailable: BTreeMap<ChoiceWord, UnavailableReason>,
    groups: BTreeMap<ChoiceWord, ChoiceGroup>,
    places: BTreeMap<ChoiceWord, ChoicePlace>,
}

/// The engine that runs `model` on this computer, when inferd starts it. A runtime the person
/// runs themselves, or an engine they attached, is not one of ours to name.
fn engine_of(model: &LocalModel) -> Option<EngineName> {
    match (&model.attached, model.loopback) {
        (None, None) => Some(EngineName(Engine::of(model.profile.kind).name().to_owned())),
        _ => None,
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
                key.push(self.pool_spec(kind, &hosted));
            }
        }
        if let Some(keys) = &self.keys {
            key.extend(keys.schema());
        }
        LiveSchema { version: 1, key }
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        if let Some(kind) = parse_pool_path(&key.0) {
            let pool = self.current_pool(kind);
            return Ok(toml::Value::Array(
                pool.into_iter().map(toml::Value::String).collect(),
            ));
        }
        let Some((kind, tier)) = parse_model_path(&key.0) else {
            return match &self.keys {
                Some(keys) if keys.handles(&key.0) => keys.get(&key.0).await,
                _ => Err(LiveError::UnknownKey(key.0.clone())),
            };
        };
        Ok(toml::Value::String(self.current(kind, tier)))
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        if let Some(kind) = parse_pool_path(&key.0) {
            return self.set_pool(kind, value).await;
        }
        let Some((slot, tier)) = parse_model_path(&key.0) else {
            return match &self.keys {
                Some(keys) if keys.handles(&key.0) => keys.set(&key.0, value).await,
                _ => Err(LiveError::UnknownKey(key.0.clone())),
            };
        };
        let kind = slot;
        let toml::Value::String(text) = value else {
            return Err(LiveError::BadValue("a model is text".into()));
        };
        let hosted = self.hosted(kind).await;
        let known = text.is_empty() || text == AUTO || self.choosable(kind, &text, &hosted);
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
    let keys = module.keys.clone();
    let served = serve(connection, INFERENCE_SETTINGS_PATH, module)
        .await
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    // The account rows announce what changed through the served module.
    if let Some(keys) = keys {
        keys.bind(served);
    }
    Ok(())
}

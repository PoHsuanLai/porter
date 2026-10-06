//! `org.quire.SettingsModule1` at `/org/quire/Inference1/settings` (design/22 section 9.4): the
//! `ai.model.<kind>.<tier>` picker, whose choices are the models inferd knows now. Only the
//! `Settings` role may `Describe`, `Get` or `Set`. A `Set` validates the model, writes it to
//! `inferd.toml` at that path and puts it in force; the next session is routed by it.

use super::file::Reload;
use super::keys::{KINDS, TIERS, kinds_of, model_path, parse_model_path};
use super::resolve::{model_text, parse_model, slot_value};
use crate::peers::{Peers, Role};
use ds_settings::live::{Access, Caller, LiveError, LiveModule, LiveSchema, Verdict, serve};
use ds_settings::schema::{
    AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label, Page, Section, WordLabels,
};
use porter_core::Tier;
use porter_dbus::INFERENCE_SETTINGS_PATH;
use porter_infer::{AiKind, tier_label};

/// The picker value that is Automatic.
const AUTO: &str = "auto";

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
    fn models_for(&self, kind: AiKind) -> Vec<String> {
        self.reload
            .engines()
            .listed()
            .into_iter()
            .filter(|one| kinds_of(&one.card).contains(&kind))
            .map(|one| {
                model_text(&porter_infer::ModelRef {
                    account: one.card.account,
                    model: one.card.model,
                })
            })
            .collect()
    }

    fn current(&self, kind: AiKind, tier: Tier) -> String {
        slot_value(&self.reload.engines().settings().tiers, kind, tier)
    }

    fn spec(&self, kind: AiKind, tier: Tier) -> KeySpec {
        let current = self.current(kind, tier);
        let mut variants = vec![String::new(), AUTO.to_owned()];
        variants.extend(self.models_for(kind));
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
        KeySpec {
            path: KeyPath(model_path(kind, tier)),
            kind: KeyKind::Menu { variants },
            default: toml::Value::String(String::new()),
            label: Label(format!("{}: {}", kind_label(kind), tier_label(tier))),
            help: Help(
                "The model used for this kind of work at this tier. Automatic picks one that \
                 is allowed and already loaded when it can."
                    .to_owned(),
            ),
            page: Page::Intelligence,
            section: Section("Models".to_owned()),
            exposure: Exposure::Basic,
            labels,
            agent: AgentSetting::HandsOff,
        }
    }
}

fn kind_label(kind: AiKind) -> &'static str {
    match kind {
        AiKind::Llm => "Language",
        AiKind::ComputerUse => "Computer use",
        AiKind::Embeddings => "Embeddings",
        AiKind::SpeechIn => "Speech to text",
        AiKind::SpeechOut => "Text to speech",
        AiKind::ImageGen => "Images",
        AiKind::Rerank => "Reranking",
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
        let known = |kind: AiKind| {
            !self.models_for(kind).is_empty()
                || TIERS
                    .iter()
                    .any(|tier| !self.current(kind, *tier).is_empty())
        };
        LiveSchema {
            version: 1,
            key: KINDS
                .into_iter()
                .filter(|kind| known(*kind))
                .flat_map(|kind| TIERS.map(|tier| self.spec(kind, tier)))
                .collect(),
        }
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        let (kind, tier) =
            parse_model_path(&key.0).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        Ok(toml::Value::String(self.current(kind, tier)))
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        let (kind, _) =
            parse_model_path(&key.0).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        let toml::Value::String(text) = value else {
            return Err(LiveError::BadValue("a model is text".into()));
        };
        let known = text.is_empty()
            || text == AUTO
            || parse_model(&text).is_some_and(|_| self.models_for(kind).contains(&text));
        if !known {
            return Err(LiveError::BadValue(format!(
                "{text}: not a model inferd knows for {}",
                kind.slug()
            )));
        }
        let _one_writer = self.writing.lock().await;
        self.reload
            .file()
            .set(&key.0, toml::Value::String(text))
            .map_err(failed)?;
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

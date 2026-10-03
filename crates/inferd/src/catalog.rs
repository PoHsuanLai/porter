//! The model catalog as porter sees it: stoker's `catalog/*.toml` files, read from the system
//! and user directories, become `Claim`s at `Provenance::Curated` for the `local` provider
//! (`Discovery::Supervised`). The parsing and the merge are stoker's `model-catalog`; this module
//! reads the directories and maps an entry's capabilities to porter's vocabulary.
//!
//! An entry with the `embeddings` role makes a claim only when an [`EmbedSpec`] names it: the
//! catalog entry has no field for the vector length, the batch limit or the query and document
//! prefixes (stoker interface ask: `ModelEntry.embed: Option<EmbedCaps>`), so inferd's own
//! configuration supplies them until it has.

use model_catalog::{CatalogKind, ModelEntry, merge_catalogs, parse_entry};
use cua_action::CuaDialect;
use model_provider::{Caps, Constraint, CuaSupport, InputKind, Support, ToolSupport, Zoom};
use porter_core::capability::{
    Capability, CuaBatching, CuaCap, CuaEnv, EmbedCap, EmbedPrompts, LanguageSet, LanguageTag,
    LlmCap, LlmFeature, LlmWire, Modality, Offered, PrefixText, SpeechCap, SpeechMode,
};
use porter_core::{Claim, Count, Dims, ModelId, Offer, Provenance, Px, Subject, Tokens};
use serde::{Deserialize, Serialize};
use speech_provider::{LangSet, SpeechDir};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use vision_prep::ResizeRule;

/// Where the catalog files live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDirs {
    /// `/usr/share/stoker/catalog`.
    pub system: PathBuf,
    /// `$XDG_DATA_HOME/stoker/catalog`; a file here replaces the system file of the same id.
    pub user: PathBuf,
}

/// What an embedding model's catalog entry does not say (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedSpec {
    /// The catalog id of the entry this describes.
    pub model: String,
    /// The vector length.
    pub dims: u32,
    /// The longest input, in tokens.
    pub max_input: u32,
    /// The most texts in one request.
    pub max_batch: u32,
    /// What is put before a query.
    pub query_prefix: String,
    /// What is put before a document.
    pub document_prefix: String,
}

/// A catalog file that was not used, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The file (or, for a bad id, the entry's id).
    pub what: String,
    /// The reason, as text for the log.
    pub why: String,
}

/// Every entry the directories give, and the files that were refused.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    /// The merged entries: system first, then user ids that are new.
    pub entries: Vec<ModelEntry>,
    /// Files that did not read or parse.
    pub skipped: Vec<Skipped>,
}

/// Reads the system and user directories and merges them (a user file replaces the system file
/// of the same id). A missing directory is empty; a file that does not parse is skipped.
pub fn read_catalog(dirs: &CatalogDirs) -> Catalog {
    let (system, mut skipped) = read_dir(&dirs.system);
    let (user, more) = read_dir(&dirs.user);
    skipped.extend(more);
    Catalog {
        entries: merge_catalogs(system, user),
        skipped,
    }
}

fn read_dir(dir: &Path) -> (Vec<ModelEntry>, Vec<Skipped>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_entry(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(entry) => entries.push(entry),
            Err(why) => skipped.push(Skipped {
                what: path.display().to_string(),
                why,
            }),
        }
    }
    (entries, skipped)
}

/// The claims the catalog makes for the local account: one per model and capability, at the
/// provenance the catalog gives (`Curated`). Embedding models are left out (see [`claims_of`]).
pub fn local_claims(dirs: &CatalogDirs) -> Vec<Claim> {
    read_catalog(dirs)
        .entries
        .iter()
        .flat_map(|entry| claims_of(entry, &[]))
        .collect()
}

/// The claims of one entry: a language model, a computer-use model, a speech model, an
/// embedding model that `embeds` names. An entry whose id is not a model id makes none.
pub fn claims_of(entry: &ModelEntry, embeds: &[EmbedSpec]) -> Vec<Claim> {
    let Ok(model) = ModelId::parse(&entry.id.0) else {
        return Vec::new();
    };
    capabilities_of(entry, embeds)
        .into_iter()
        .map(|capability| Claim {
            subject: Subject::Model(model.clone()),
            offer: Offer::Present(capability),
            provenance: Provenance::Curated,
        })
        .collect()
}

/// What the entry's roles make of it, in role order.
pub fn capabilities_of(entry: &ModelEntry, embeds: &[EmbedSpec]) -> Vec<Capability> {
    entry
        .roles
        .iter()
        .filter_map(|role| match (role, &entry.caps) {
            (CatalogKind::Llm, Some(caps)) => Some(Capability::Llm(llm_cap(caps, entry))),
            (CatalogKind::ComputerUse, Some(caps)) => cua_cap(caps).map(Capability::ComputerUse),
            (CatalogKind::Embeddings, _) => embeds
                .iter()
                .find(|spec| spec.model == entry.id.0)
                .map(|spec| Capability::Embeddings(embed_cap(spec))),
            (CatalogKind::SpeechIn | CatalogKind::SpeechOut, _) => speech_cap(entry),
            _ => None,
        })
        .collect()
}

fn llm_cap(caps: &Caps, entry: &ModelEntry) -> LlmCap {
    let llama = entry
        .engines
        .iter()
        .any(|engine| engine.kind == model_catalog::EngineKind::LlamaServer);
    let features: [(bool, LlmFeature); 6] = [
        (true, LlmFeature::Chat),
        (caps.tools != ToolSupport::Absent, LlmFeature::Tools),
        (caps.inputs.contains(&InputKind::Image), LlmFeature::Vision),
        (
            caps.output.contains(&Constraint::JsonSchema),
            LlmFeature::StructuredOutput,
        ),
        (caps.reasoning == Support::Present, LlmFeature::Reasoning),
        (llama, LlmFeature::PromptCache),
    ];
    LlmCap {
        features: features
            .into_iter()
            .filter_map(|(has, feature)| has.then_some(feature))
            .collect(),
        context: Tokens(caps.context.0),
        max_output: Tokens(caps.max_output.0),
        wire: LlmWire::ChatCompletions,
    }
}

/// A computer-use claim needs a tool dialect (the text and wire dialects have no runner yet: the
/// prompt and parse of a step are `cua-session`'s, and `cua_step` serves only the tool calls)
/// and a model that sees images.
fn cua_cap(caps: &Caps) -> Option<CuaCap> {
    let CuaSupport::Dialect {
        dialect: CuaDialect::Tool(_),
        batching,
        zoom,
    } = caps.computer_use
    else {
        return None;
    };
    Some(CuaCap {
        environments: BTreeSet::from([CuaEnv::Desktop]),
        batching: match batching {
            model_provider::Batching::One => CuaBatching::One,
            model_provider::Batching::Many => CuaBatching::Many,
        },
        zoom: match zoom {
            Zoom::Absent => Offered::Absent,
            Zoom::Native => Offered::Present,
        },
        max_image: Px(longest_side(&caps.images.rule)),
        wire: LlmWire::ChatCompletions,
    })
}

/// The longest side the rule lets an image have: the cap's own, or the side of a square of the
/// largest area.
fn longest_side(rule: &ResizeRule) -> u32 {
    let square = |pixels: u64| u32::try_from(pixels.isqrt()).unwrap_or(u32::MAX);
    match rule {
        ResizeRule::SmartResize { max_pixels, .. } => square(max_pixels.0),
        ResizeRule::LongEdge { max_edge, .. } => max_edge.0,
        ResizeRule::Identity => 0,
    }
}

fn speech_cap(entry: &ModelEntry) -> Option<Capability> {
    let speech = entry.speech.as_ref()?;
    let mode = match speech.dir() {
        SpeechDir::In => SpeechMode::Stt,
        SpeechDir::Out => SpeechMode::Tts,
    };
    let languages = match &speech.langs {
        LangSet::Any => LanguageSet::Any,
        LangSet::Listed(langs) => LanguageSet::Listed(
            langs
                .iter()
                .filter_map(|lang| LanguageTag::parse(lang.as_str()).ok())
                .collect(),
        ),
    };
    Some(Capability::Speech(SpeechCap {
        modes: BTreeSet::from([mode]),
        languages,
    }))
}

fn embed_cap(spec: &EmbedSpec) -> EmbedCap {
    EmbedCap {
        dims: Dims(spec.dims),
        modalities: BTreeSet::from([Modality::Text]),
        max_input: Tokens(spec.max_input),
        max_batch: Count(spec.max_batch),
        prompts: Box::new(EmbedPrompts {
            query: PrefixText(spec.query_prefix.clone()),
            document: PrefixText(spec.document_prefix.clone()),
        }),
    }
}

#[cfg(test)]
mod tests;

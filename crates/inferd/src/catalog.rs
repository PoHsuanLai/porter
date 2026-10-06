//! The model catalog as porter sees it: stoker's `catalog/*.toml` files, read from the system
//! and user directories, become `Claim`s at `Provenance::Curated` for the `local` provider
//! (`Discovery::Supervised`). The parsing and the merge are stoker's `model-catalog`; this module
//! reads the directories and maps an entry's capabilities to porter's vocabulary.
//!
//! An entry with the `embeddings` role makes a claim when it has the `embed` table
//! (`ModelEntry.embed`: the vector length, the batch and input limits, the query and document
//! prefixes); without the table it makes none. inferd keeps no override of its own: the catalog is
//! the one place a model's details are written, and a user who needs other prefixes writes a
//! catalog file of the same id in `$XDG_DATA_HOME/stoker/catalog`.

use cua_action::CuaDialect;
use model_catalog::{CatalogKind, ModelEntry, merge_catalogs, parse_entry};
use model_provider::{
    Caps, Constraint, CuaSupport, EmbedCaps, InputKind, Support, ToolSupport, Zoom,
};
use porter_core::capability::{
    Capability, CuaBatching, CuaCap, CuaEnv, EmbedCap, EmbedPrompts, LanguageSet, LanguageTag,
    LlmCap, LlmFeature, LlmWire, Modality, Offered, PrefixText, SpeechCap, SpeechMode,
};
use porter_core::{Claim, Count, Dims, ModelId, Offer, Provenance, Px, Subject, Tokens};
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

/// One entry's text as an entry (stoker's parser, so a test or a caller need not name it).
pub fn parse_entry_text(text: &str) -> Result<ModelEntry, model_catalog::CatalogError> {
    parse_entry(text)
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

/// The claims the catalog makes for the local account: one per model and capability of an entry
/// that runs on this computer, at the provenance the catalog gives (`Curated`); a hosted entry is
/// not the local account's.
pub fn local_claims(dirs: &CatalogDirs) -> Vec<Claim> {
    read_catalog(dirs)
        .entries
        .iter()
        .filter(|entry| entry.locality.is_on_device())
        .flat_map(claims_of)
        .collect()
}

/// The claims of one entry: a language model, a computer-use model, a speech model, an
/// embedding model with an `embed` table. An entry whose id is not a model id makes none.
pub fn claims_of(entry: &ModelEntry) -> Vec<Claim> {
    let Ok(model) = ModelId::parse(&entry.id.0) else {
        return Vec::new();
    };
    capabilities_of(entry)
        .into_iter()
        .map(|capability| Claim {
            subject: Subject::Model(model.clone()),
            offer: Offer::Present(capability),
            provenance: Provenance::Curated,
        })
        .collect()
}

/// What the entry's roles make of it, in role order.
pub fn capabilities_of(entry: &ModelEntry) -> Vec<Capability> {
    entry
        .roles
        .iter()
        .filter_map(|role| match (role, &entry.caps) {
            (CatalogKind::Llm, Some(caps)) => Some(Capability::Llm(llm_cap(caps, entry))),
            (CatalogKind::ComputerUse, Some(caps)) => cua_cap(caps).map(Capability::ComputerUse),
            (CatalogKind::Embeddings, _) => entry
                .embed
                .as_ref()
                .map(|embed| Capability::Embeddings(embed_cap(embed))),
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

/// A computer-use claim needs a dialect an engine on this computer can speak (a tool call or the
/// text of UI-TARS: `cua-session` builds the prompt and reads the reply of both; a vendor's wire
/// needs the vendor's backend) and a model that sees images.
fn cua_cap(caps: &Caps) -> Option<CuaCap> {
    let CuaSupport::Dialect {
        dialect: CuaDialect::Tool(_) | CuaDialect::Text(_),
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

fn embed_cap(embed: &EmbedCaps) -> EmbedCap {
    EmbedCap {
        dims: Dims(embed.dims.0),
        modalities: BTreeSet::from([Modality::Text]),
        max_input: Tokens(embed.max_input.0),
        max_batch: Count(embed.max_batch.0),
        prompts: Box::new(EmbedPrompts {
            query: PrefixText(embed.prompts.query.0.clone()),
            document: PrefixText(embed.prompts.document.0.clone()),
        }),
    }
}

#[cfg(test)]
mod tests;

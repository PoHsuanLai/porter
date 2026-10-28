//! Which models have shown they can make a valid tool call, kept by model id.
//!
//! Automatic skips a model known to fail when the request offers tools, and a named model is
//! never skipped (`TierMap::for_tools`). The result of the check comes from two places: the
//! canned request (`porter_infer::probe_request`), run once on a model of this computer the first
//! time Automatic considers it for a request with tools (`Engines::prove_tools`),
//! and a real turn that offered tools and ended unreadable ([`ToolWatch`]). A hosted model is not
//! probed: it is taken as capable until a real turn says otherwise, since a probe would be a
//! request on the person's account for nothing they asked.
//!
//! The results are a cache in `tool-proofs.json` beside the audit trail: model ids and a word,
//! never a prompt. A file that cannot be read is an empty cache, and the checks run again.

use porter_capability::LlmFeature;
use porter_fs::atomic::AtomicWrite;
use porter_infer::{InferReply, ModelError, ModelRef, ServedBy, ToolProof, Why};
use porter_match::Need;
use porter_router::carried::Carried;
use porter_router::seams::AuditSink;
use porter_router::session::SessionSpec;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// Whether a request of `need` offers tools.
pub fn asks_for_tools(need: &Need) -> bool {
    matches!(need, Need::Llm(llm) if llm.features.contains(&LlmFeature::Tools))
}

#[derive(Debug, Serialize, Deserialize)]
struct Row {
    model: ModelRef,
    proof: ToolProof,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    rows: Vec<Row>,
}

#[derive(Debug, Default)]
struct Held {
    proofs: BTreeMap<ModelRef, ToolProof>,
    file: Option<PathBuf>,
}

/// What is known of each model, shared by every clone.
#[derive(Debug, Clone, Default)]
pub struct ToolProofs {
    held: Arc<Mutex<Held>>,
}

impl ToolProofs {
    /// Nothing known, and nothing kept across a restart.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// What `file` holds, and every record written back to it.
    pub fn open(file: PathBuf) -> Self {
        let document = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<Document>(&text).ok())
            .unwrap_or_default();
        Self {
            held: Arc::new(Mutex::new(Held {
                proofs: document
                    .rows
                    .into_iter()
                    .map(|row| (row.model, row.proof))
                    .collect(),
                file: Some(file),
            })),
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What is known of `model`.
    pub fn get(&self, model: &ModelRef) -> Option<ToolProof> {
        self.held().proofs.get(model).copied()
    }

    /// The models known to fail.
    pub fn failed(&self) -> BTreeSet<ModelRef> {
        self.held()
            .proofs
            .iter()
            .filter(|(_, proof)| **proof == ToolProof::Failed)
            .map(|(model, _)| model.clone())
            .collect()
    }

    /// Records what was found of `model`, over what was known.
    pub fn record(&self, model: ModelRef, proof: ToolProof) {
        let mut held = self.held();
        held.proofs.insert(model, proof);
        let Some(file) = &held.file else { return };
        let document = Document {
            rows: held
                .proofs
                .iter()
                .map(|(model, proof)| Row {
                    model: model.clone(),
                    proof: *proof,
                })
                .collect(),
        };
        let write = || -> std::io::Result<()> {
            let text = serde_json::to_string(&document).map_err(std::io::Error::other)?;
            AtomicWrite::PRIVATE.write(file, text.as_bytes())
        };
        if let Err(why) = write() {
            eprintln!("inferd: tool proofs: {}: {why}", file.display());
        }
    }
}

/// An audit sink that also notes a model whose turn offered tools and ended unreadable, then
/// hands the turn to the sink it wraps unchanged.
#[derive(Debug)]
pub struct ToolWatch<A> {
    inner: A,
    proofs: ToolProofs,
}

impl<A> ToolWatch<A> {
    /// `inner`, noting failures in `proofs`.
    pub fn new(inner: A, proofs: ToolProofs) -> Self {
        Self { inner, proofs }
    }

    fn note(&self, spec: &SessionSpec, served: &ServedBy, reply: &InferReply) {
        if asks_for_tools(&spec.need)
            && matches!(reply, InferReply::Failed(ModelError::Unparseable))
        {
            let model = ModelRef {
                account: served.account.clone(),
                model: served.model.clone(),
            };
            self.proofs.record(model, ToolProof::Failed);
        }
    }
}

impl<A: AuditSink> AuditSink for ToolWatch<A> {
    fn record(&self, spec: &SessionSpec, served: &ServedBy, reply: &InferReply, carried: &Carried) {
        self.note(spec, served, reply);
        self.inner.record(spec, served, reply, carried);
    }

    fn record_why(
        &self,
        spec: &SessionSpec,
        served: &ServedBy,
        reply: &InferReply,
        carried: &Carried,
        why: &Why,
    ) {
        self.note(spec, served, reply);
        self.inner.record_why(spec, served, reply, carried, why);
    }
}

#[cfg(test)]
mod tests;

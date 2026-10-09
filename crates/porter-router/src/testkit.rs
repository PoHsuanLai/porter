//! Helpers for tests of this crate and of the ones built on it (feature `testing`): a scratch
//! directory that removes itself, the catalog entries the tests use, the local models they make,
//! and a model of an account that is not on this computer.

pub mod entries;

use crate::catalog::parse_entry_text;
use crate::local::{EngineConfig, LocalModel, build};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// A new, empty directory named for `name`.
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "inferd-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The three test models (`tiny-chat`, `tiny-embed`, `tiny-cua`), with engine programs
/// configured and the weights directories created under `scratch`.
pub fn models(scratch: &Scratch) -> Vec<LocalModel> {
    let entries: Vec<_> = [entries::chat(), entries::embed(), entries::cua()]
        .iter()
        .map(|text| parse_entry_text(text).expect("entry"))
        .collect();
    let config = EngineConfig {
        vllm_python: Some(PathBuf::from("/nonexistent/python")),
        llama_server: Some(PathBuf::from("/nonexistent/llama-server")),
        speech_host: None,
        kokoro_python: None,
        hf_cache: scratch.path().join("hf"),
        ..EngineConfig::default()
    };
    let models = build(&entries, &config, &scratch.path().join("s"));
    for model in &models {
        std::fs::create_dir_all(model.spec.unit.sandbox.read.first().expect("bind"))
            .expect("weights");
    }
    models
}

/// A model of an account that is not on this computer, for routing tests that must not reach it.
pub fn cloud_card() -> porter_infer::ModelCard {
    use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
    use porter_core::{AccountId, Billing, Locality, ModelId, Tokens};
    porter_infer::ModelCard {
        account: AccountId::parse("anthropic").expect("id"),
        model: ModelId::parse("sonnet").expect("id"),
        locality: Locality::Cloud { region: None },
        billing: Billing::PlanBudget,
        capabilities: vec![Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(100_000),
            max_output: Tokens(4096),
            wire: LlmWire::Messages,
        })],
    }
}

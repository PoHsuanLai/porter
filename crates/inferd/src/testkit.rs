//! Helpers the unit tests share: a scratch directory that removes itself, and the local models of
//! the test catalog entries.

use crate::catalog::{EmbedSpec, parse_entry_text};
use crate::entries;
use crate::local::{EngineConfig, LocalModel, build};
use engine_supervisor::{EngineHost, EngineId, ExitCode, HostError, UnitSpec};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "inferd-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn embed_spec() -> EmbedSpec {
    EmbedSpec {
        model: "tiny-embed".into(),
        dims: 4,
        max_input: 512,
        max_batch: 2,
        query_prefix: "search_query: ".into(),
        document_prefix: "search_document: ".into(),
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
    };
    let models = build(
        &entries,
        &[embed_spec()],
        &config,
        &scratch.path().join("s"),
    );
    for model in &models {
        std::fs::create_dir_all(model.spec.unit.sandbox.read.first().expect("bind"))
            .expect("weights");
    }
    models
}

/// A host that records what it was asked and lets an engine "exit" when it is stopped.
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    pub spawned: Arc<Mutex<Vec<EngineId>>>,
    pub stopped: Arc<Mutex<Vec<EngineId>>>,
    pub gone: Arc<Notify>,
}

impl EngineHost for Recorder {
    async fn spawn(&self, id: &EngineId, _: &UnitSpec) -> Result<(), HostError> {
        self.spawned.lock().expect("lock").push(id.clone());
        Ok(())
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        self.stopped.lock().expect("lock").push(id.clone());
        self.gone.notify_waiters();
        Ok(())
    }

    async fn exited(&self, _: &EngineId) -> ExitCode {
        self.gone.notified().await;
        ExitCode(0)
    }
}

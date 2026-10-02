//! Whether a model can answer now (design/31 §3.3).

use porter_core::Permille;
use serde::{Deserialize, Serialize};

/// How ready one model is. Ordered by how soon it can answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Readiness {
    /// Loaded and answering.
    Ready,
    /// Loading into memory.
    Loading,
    /// Installed and stopped: a request starts it.
    Loadable,
    /// Its weights are coming down.
    Downloading(Permille),
    /// Not installed, but the catalog can fetch it.
    Downloadable,
    /// Not available at all.
    Unavailable,
}

impl Readiness {
    /// The slug `Inference1.Prepare` answers with.
    pub fn slug(self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::Loading => "loading",
            Readiness::Loadable => "loadable",
            Readiness::Downloading(_) => "downloading",
            Readiness::Downloadable => "downloadable",
            Readiness::Unavailable => "unavailable",
        }
    }
}

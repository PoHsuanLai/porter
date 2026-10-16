//! Which engine a row, a status or a request is about.

/// An engine this computer can run models with and that we can fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EngineId {
    /// llama.cpp's server.
    LlamaCpp,
    /// vLLM, in a Python environment of its own.
    Vllm,
    /// Strata, run from its own folder.
    Strata,
}

impl EngineId {
    /// Every engine, in the picker's order.
    pub const ALL: [EngineId; 3] = [Self::LlamaCpp, Self::Vllm, Self::Strata];

    /// The id on the bus and in a directory name.
    pub fn slug(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama_cpp",
            Self::Vllm => "vllm",
            Self::Strata => "strata",
        }
    }

    /// The engine a slug names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.slug() == text)
    }

    /// What a person calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama.cpp",
            Self::Vllm => "vLLM",
            Self::Strata => "Strata",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_names_one_engine_and_nothing_else_does() {
        for engine in EngineId::ALL {
            assert_eq!(EngineId::from_slug(engine.slug()), Some(engine));
        }
        for text in [
            "",
            "LlamaCpp",
            "llama.cpp",
            "../vllm",
            "vllm ",
            "https://x/y",
        ] {
            assert_eq!(EngineId::from_slug(text), None, "{text:?}");
        }
    }
}

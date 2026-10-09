//! The seam between porter's wire types and stoker's provider types (`model-provider`):
//! `ChatRequest` to `TurnRequest`, `ToolCallPart` to `ToolCall`, stoker's `TurnEvent` to
//! `InferEvent`, `ProviderError` to `ModelError`, `StopReason` both ways. `request` is the way
//! in and `reply` the way out; the mapping of the served name to a `ModelId` is here.
//!
//! Moved out of inferd so that the process that hosts the engines itself (porter-client's
//! `engines` feature, an app with no inferd) speaks to them with the same mapping the daemon
//! does. Pure: no runtime, no socket.

mod reply;
mod request;
mod scores;

pub use reply::{Gathered, event, model_error, stop, stop_back, usage, vectors, width};
pub use request::{
    BridgeError, DefaultSampling, Frames, MAX_ATTACHMENT, Target, chat_turn_for, embed_turns_for,
    extras, image_input, task_turn_for,
};
pub use scores::{NoScores, choice_scores, option_scores, turn_scores};

use model_provider::{Attempt, RetryPolicy, WaitMs};
use porter_core::{CoreError, ModelId};

/// How a request to an engine is retried, by the daemon and by an app that hosts the engines
/// itself alike: three attempts, a quarter of a second doubling to four seconds.
pub const ENGINE_RETRY: RetryPolicy = RetryPolicy {
    attempts: Attempt(3),
    base: WaitMs(250),
    cap: WaitMs(4000),
};

/// The porter `ModelId` for the name an engine serves a model under (`Hcompany/Holo-3.1-4B`
/// becomes `hcompany-holo-3.1-4b`): lower case, every character outside the id grammar becomes
/// `-`, runs collapse, ends are trimmed, at most 64 bytes.
pub fn model_id_of(served_name: &str) -> Result<ModelId, CoreError> {
    let mut slug = String::new();
    for c in served_name.chars().flat_map(char::to_lowercase) {
        let keep = c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_');
        match (keep, slug.ends_with('-')) {
            (true, _) => slug.push(c),
            (false, false) => slug.push('-'),
            (false, true) => {}
        }
    }
    let trimmed = slug.trim_matches(|c| c == '-' || c == '.' || c == '_');
    let end = trimmed
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| end <= 64)
        .last()
        .unwrap_or(0);
    ModelId::parse(&trimmed[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// inferd and porter-client's engines both retry with this one policy.
    #[test]
    fn the_engine_retry_is_three_tries_from_a_quarter_second_to_four() {
        assert_eq!(ENGINE_RETRY.attempts, Attempt(3));
        assert_eq!(ENGINE_RETRY.base, WaitMs(250));
        assert_eq!(ENGINE_RETRY.cap, WaitMs(4000));
    }

    #[test]
    fn served_names_become_model_ids() {
        let cases = [
            ("llama3.2", Some("llama3.2")),
            ("Hcompany/Holo-3.1-4B", Some("hcompany-holo-3.1-4b")),
            ("  Nemotron 3.5 ASR (int8) ", Some("nemotron-3.5-asr-int8")),
            ("a//b", Some("a-b")),
            ("///", None),
            ("", None),
        ];
        for (name, expected) in cases {
            let got = model_id_of(name).ok().map(|id| id.as_str().to_owned());
            assert_eq!(got.as_deref(), expected, "{name:?}");
        }
        let long = "x".repeat(100);
        assert_eq!(model_id_of(&long).expect("id").as_str().len(), 64);
    }
}

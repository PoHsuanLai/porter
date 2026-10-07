//! Porter's requests as stoker's turns over a model of inferd's book: the turn target of a
//! `LocalModel` (its catalog entry's default sampling and output limit, its engine's flavor), and
//! the embedding limits of its entry. The mapping is `porter_bridge`'s.

use crate::local::LocalModel;
use model_provider as sp;
use porter_bridge::{
    BridgeError, DefaultSampling, Frames, Target, chat_turn_for, embed_turns_for, task_turn_for,
};
use porter_infer as pi;

/// The turn target of a model on this computer.
pub fn local_target(model: &LocalModel) -> Target {
    Target {
        name: model.name.clone(),
        sampling: DefaultSampling::Entry(model.entry.sampling),
        max_output: model.caps().map(|caps| caps.max_output).unwrap_or_default(),
        flavor: model.flavor,
    }
}

/// The turn for a chat request. Sampling and the output limit the request leaves open come from
/// the model's catalog entry.
pub fn chat_turn(
    model: &LocalModel,
    request: &pi::ChatRequest,
    frames: &Frames,
) -> Result<sp::TurnRequest, BridgeError> {
    chat_turn_for(&local_target(model), request, frames)
}

/// A task as a chat turn: its instruction, then the text.
pub fn task_turn(
    model: &LocalModel,
    request: &pi::TaskRequest,
    tier: porter_core::Tier,
) -> Result<sp::TurnRequest, BridgeError> {
    task_turn_for(&local_target(model), request, tier)
}

/// The embedding turns of a request: the model's prefix for the request's role goes before every
/// text, and the texts are cut into batches of the model's limit.
pub fn embed_turns(
    model: &LocalModel,
    request: &pi::EmbedRequest,
) -> Result<Vec<sp::EmbedTurn>, BridgeError> {
    let embed = model.embed().ok_or(BridgeError::Unsupported)?;
    embed_turns_for(&model.name, embed, request)
}

#[cfg(test)]
mod tests;

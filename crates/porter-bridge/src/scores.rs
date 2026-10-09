//! The shares of a `Choice`'s options, from the engine's first-token log-probabilities.
//!
//! The way in: a chat request whose shape is a `Choice` of two or more options and whose
//! `scores` knob is set asks the engine for the likeliest first tokens ([`choice_scores`]). The
//! way out: the tokens the engine reported are renormalised over the declared options
//! ([`option_scores`]) into one `Permille` share per option, in the declared order, adding up to
//! exactly 1000.
//!
//! Stoker's [`FirstTokenLogprobs::option_permille`] does the arithmetic and states the rules once:
//! a token counts for the option whose text starts with it; an option missing from the top-k has
//! mass 0; the largest remainder settles the rounding (ties go to the earlier option); and the
//! answer is `None` when a token with mass starts two options (first-token scores cannot tell
//! them apart; stoker carries no whole-sequence log-probabilities to fall back on), when no
//! option has any mass, or with fewer than two options. A NaN never reaches it (stoker drops the
//! entry) and a `-inf` is a mass of 0. This module only maps its answer onto porter's types, so
//! the two `Permille` types meet in one place, and checks the result once more.

use model_provider::{self as sp, FirstTokenLogprobs};
use porter_core::Permille;
use porter_infer as pi;

/// The widest top-k an engine is asked for (the OpenAI-compatible APIs stop at 20).
const MOST: u32 = pi::MOST_CANDIDATES;

/// Why a reply has no scores, for a note in the daemon's log. Never an error for the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoScores {
    /// The engine reported no first-token probabilities (hosted providers have none, and an
    /// engine can leave them out of a stream).
    #[error("the engine gave no first-token probabilities")]
    EngineGaveNone,
    /// It did, but they cannot be turned into shares: two options share a first token, none of
    /// the options is among the likeliest tokens, or nothing is left of them.
    #[error("the first-token probabilities cannot tell the options apart")]
    Unusable,
}

/// What a turn asks the engine for: the first-token candidates when the app set the knob and the
/// shape is a `Choice` of two or more options, else nothing.
pub fn choice_scores(control: &pi::ChatControl, shape: &sp::OutputShape) -> sp::ChoiceScores {
    match (control.scores, shape) {
        (pi::Knob::Set(options), sp::OutputShape::Choice(choices)) if choices.len() >= 2 => {
            sp::ChoiceScores::FirstToken {
                top_k: sp::Count(options.top_k.0.clamp(1, MOST)),
            }
        }
        _ => sp::ChoiceScores::Off,
    }
}

/// The options a turn declared, when it asked for their scores.
pub(crate) fn asked_options(turn: &sp::TurnRequest) -> Option<Vec<String>> {
    match (&turn.choice_scores, &turn.output) {
        (sp::ChoiceScores::FirstToken { .. }, sp::OutputShape::Choice(choices)) => {
            Some(choices.clone())
        }
        _ => None,
    }
}

/// The shares a turn asked for, from the first token its engine reported: `None` when the turn
/// did not ask, else the shares or why there are none.
pub fn turn_scores(
    turn: &sp::TurnRequest,
    first_token: Option<&FirstTokenLogprobs>,
) -> Option<Result<pi::OptionScores, NoScores>> {
    let options = asked_options(turn)?;
    Some(option_scores(&options, first_token))
}

/// The share of each of `options` in the order given, adding up to 1000, or why there is none.
pub fn option_scores(
    options: &[String],
    first_token: Option<&FirstTokenLogprobs>,
) -> Result<pi::OptionScores, NoScores> {
    let first_token = first_token.ok_or(NoScores::EngineGaveNone)?;
    let names: Vec<&str> = options.iter().map(String::as_str).collect();
    let shares = first_token
        .option_permille(&names)
        .ok_or(NoScores::Unusable)?;
    pi::OptionScores::new(
        options
            .iter()
            .zip(shares)
            .map(|(option, share)| pi::OptionScore {
                option: option.clone(),
                share: Permille(u32::from(share.0)),
            })
            .collect(),
    )
    .map_err(|_| NoScores::Unusable)
}

#[cfg(test)]
mod tests;

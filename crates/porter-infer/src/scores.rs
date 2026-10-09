//! How likely each option of a `Choice` reply was, from the same call that chose one: the
//! opt-in an app writes on its [`crate::ChatControl`] ([`ScoreOptions`]) and the shares that come
//! back on the [`crate::ChatReply`] ([`OptionScores`]).
//!
//! The shares are the model's own first-token probabilities, renormalised over the options the
//! request declared; they are a hint for a caller that wants to know how sure the model was, not
//! a calibrated probability. They are only ever asked for, and only ever answered, for a request
//! whose shape is [`crate::ReplyShape::Choice`]; on any other shape the knob is ignored.

use porter_core::{Count, Permille};
use serde::{Deserialize, Serialize};

/// The most first-token candidates an engine is asked for, and the number asked when an app
/// does not choose: the widest the engines take.
pub const MOST_CANDIDATES: u32 = 20;

/// What to ask the engine for, when the app wants the shares of a `Choice`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScoreOptions {
    /// How many of the likeliest first tokens the engine reports. An option whose first token
    /// is not among them counts as 0. Held to 1 through [`MOST_CANDIDATES`] on the way to the
    /// engine; the default is the most.
    pub top_k: Count,
}

impl Default for ScoreOptions {
    fn default() -> Self {
        Self {
            top_k: Count(MOST_CANDIDATES),
        }
    }
}

/// One declared option and its share of the model's belief, in thousandths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OptionScore {
    /// The option, as the request declared it.
    pub option: String,
    /// Its share: `Permille(700)` is 70.0 per cent. At most 1000.
    pub share: Permille,
}

/// Why a set of shares is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum ScoresError {
    /// A single option has nothing to be compared with.
    #[error("fewer than two options")]
    TooFew,
    /// One share is over a whole.
    #[error("a share is over 1000")]
    OverAWhole,
    /// The shares do not add up to a whole.
    #[error("the shares add up to {0}, not 1000")]
    NotAWhole(u32),
}

/// One share per declared option, in the declared order, adding up to exactly 1000 (the bridge
/// settles rounding by the largest remainder, ties to the earlier option). Built only through
/// [`OptionScores::new`], also when read off the wire, so a value that exists is a whole.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<OptionScore>", into = "Vec<OptionScore>")]
pub struct OptionScores(Vec<OptionScore>);

impl OptionScores {
    /// The shares, checked: two or more, each at most 1000, adding up to 1000.
    pub fn new(scores: Vec<OptionScore>) -> Result<Self, ScoresError> {
        if scores.len() < 2 {
            return Err(ScoresError::TooFew);
        }
        if scores.iter().any(|score| score.share.0 > 1000) {
            return Err(ScoresError::OverAWhole);
        }
        let sum: u32 = scores.iter().map(|score| score.share.0).sum();
        if sum != 1000 {
            return Err(ScoresError::NotAWhole(sum));
        }
        Ok(Self(scores))
    }

    /// The shares, in the declared order.
    pub fn as_slice(&self) -> &[OptionScore] {
        &self.0
    }

    /// The share of `option`, if it is one of the declared options.
    pub fn share_of(&self, option: &str) -> Option<Permille> {
        self.0
            .iter()
            .find(|score| score.option == option)
            .map(|score| score.share)
    }
}

impl TryFrom<Vec<OptionScore>> for OptionScores {
    type Error = ScoresError;

    fn try_from(scores: Vec<OptionScore>) -> Result<Self, ScoresError> {
        Self::new(scores)
    }
}

impl From<OptionScores> for Vec<OptionScore> {
    fn from(scores: OptionScores) -> Self {
        scores.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(option: &str, share: u32) -> OptionScore {
        OptionScore {
            option: option.into(),
            share: Permille(share),
        }
    }

    #[test]
    fn shares_that_make_a_whole_are_kept_in_their_order() {
        let scores = OptionScores::new(vec![score("b", 300), score("a", 700)]).expect("a whole");
        assert_eq!(scores.as_slice()[0].option, "b");
        assert_eq!(scores.share_of("a"), Some(Permille(700)));
        assert_eq!(scores.share_of("c"), None);
    }

    #[test]
    fn shares_that_are_not_a_whole_are_refused() {
        assert_eq!(
            OptionScores::new(vec![score("a", 500), score("b", 499)]),
            Err(ScoresError::NotAWhole(999))
        );
        assert_eq!(
            OptionScores::new(vec![score("a", 1001), score("b", 0)]),
            Err(ScoresError::OverAWhole)
        );
        assert_eq!(
            OptionScores::new(vec![score("a", 1000)]),
            Err(ScoresError::TooFew)
        );
    }

    #[test]
    fn a_wire_value_that_is_not_a_whole_does_not_read() {
        let bad = r#"[{"option":"a","share":600},{"option":"b","share":600}]"#;
        assert!(serde_json::from_str::<OptionScores>(bad).is_err());
        let good = r#"[{"option":"a","share":600},{"option":"b","share":400}]"#;
        let scores: OptionScores = serde_json::from_str(good).expect("reads");
        assert_eq!(serde_json::to_string(&scores).expect("writes"), good);
    }
}

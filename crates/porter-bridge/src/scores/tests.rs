use super::*;
use model_provider::{Logprob, TokenLogprob};

fn options(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A first token the engine found likely with this probability.
fn token(text: &str, probability: f64) -> TokenLogprob {
    TokenLogprob {
        token: text.into(),
        logprob: Logprob::from_nats(probability.ln()).expect("a number"),
    }
}

fn first(tokens: Vec<TokenLogprob>) -> FirstTokenLogprobs {
    FirstTokenLogprobs { top: tokens }
}

fn shares(scores: &pi::OptionScores) -> Vec<u32> {
    scores.as_slice().iter().map(|one| one.share.0).collect()
}

#[test]
fn the_shares_follow_the_declared_order_and_add_up_to_a_thousand() {
    let declared = options(&["allow", "deny", "ask"]);
    // Probabilities that do not divide evenly: 0.6, 0.3, 0.1 of what is seen, plus a token that
    // starts no option (it is left out before the rest is renormalised).
    let engine = first(vec![
        token("deny", 0.3),
        token("allow", 0.6),
        token("ask", 0.1),
        token("Sure", 0.05),
    ]);
    let scores = option_scores(&declared, Some(&engine)).expect("scores");
    assert_eq!(shares(&scores), vec![600, 300, 100]);
    let names: Vec<&str> = scores
        .as_slice()
        .iter()
        .map(|one| one.option.as_str())
        .collect();
    assert_eq!(names, ["allow", "deny", "ask"]);
}

#[test]
fn rounding_goes_to_the_largest_remainder_so_the_sum_is_exact() {
    let declared = options(&["a", "b", "c"]);
    let engine = first(vec![token("a", 0.2), token("b", 0.2), token("c", 0.2)]);
    let scores = option_scores(&declared, Some(&engine)).expect("scores");
    // 333.33 each: the one spare thousandth goes to the earliest of the equal remainders.
    assert_eq!(shares(&scores), vec![334, 333, 333]);
    assert_eq!(shares(&scores).iter().sum::<u32>(), 1000);
}

#[test]
fn a_tie_between_two_options_splits_evenly() {
    let declared = options(&["yes", "no"]);
    let engine = first(vec![token("yes", 0.45), token("no", 0.45)]);
    let scores = option_scores(&declared, Some(&engine)).expect("scores");
    assert_eq!(shares(&scores), vec![500, 500]);
}

#[test]
fn options_that_share_a_first_token_cannot_be_told_apart() {
    // "Allow" and "Always" both start with the token "Al": first-token scores say nothing.
    let declared = options(&["Allow", "Always", "Deny"]);
    let engine = first(vec![token("Al", 0.7), token("Deny", 0.3)]);
    assert_eq!(
        option_scores(&declared, Some(&engine)),
        Err(NoScores::Unusable)
    );
}

#[test]
fn an_option_missing_from_the_top_k_counts_as_nothing() {
    let declared = options(&["yes", "no", "maybe"]);
    let engine = first(vec![token("yes", 0.75), token("no", 0.25)]);
    let scores = option_scores(&declared, Some(&engine)).expect("scores");
    assert_eq!(shares(&scores), vec![750, 250, 0]);
}

#[test]
fn when_no_option_is_in_the_top_k_there_are_no_scores() {
    let declared = options(&["yes", "no"]);
    let engine = first(vec![token("Maybe", 0.6), token("Perhaps", 0.4)]);
    assert_eq!(
        option_scores(&declared, Some(&engine)),
        Err(NoScores::Unusable)
    );
    assert_eq!(
        option_scores(&declared, Some(&first(vec![]))),
        Err(NoScores::Unusable)
    );
}

#[test]
fn a_nan_is_dropped_and_minus_infinity_is_no_mass() {
    // A NaN never becomes a `Logprob`: the engine side leaves the entry out.
    assert_eq!(Logprob::from_nats(f64::NAN), None);
    let declared = options(&["yes", "no"]);
    let impossible = TokenLogprob {
        token: "no".into(),
        logprob: Logprob::from_nats(f64::NEG_INFINITY).expect("-inf is a value"),
    };
    assert_eq!(impossible.logprob, Logprob::NEVER);
    let engine = first(vec![token("yes", 0.9), impossible]);
    let scores = option_scores(&declared, Some(&engine)).expect("scores");
    assert_eq!(shares(&scores), vec![1000, 0]);
    // Everything impossible leaves nothing to share.
    let only_impossible = first(vec![TokenLogprob {
        token: "yes".into(),
        logprob: Logprob::NEVER,
    }]);
    assert_eq!(
        option_scores(&declared, Some(&only_impossible)),
        Err(NoScores::Unusable)
    );
}

#[test]
fn an_engine_that_reported_nothing_is_told_apart_from_one_that_reported_nonsense() {
    let declared = options(&["yes", "no"]);
    assert_eq!(
        option_scores(&declared, None),
        Err(NoScores::EngineGaveNone)
    );
}

#[test]
fn a_single_option_has_nothing_to_be_compared_with() {
    let declared = options(&["yes"]);
    let engine = first(vec![token("yes", 0.9)]);
    assert_eq!(
        option_scores(&declared, Some(&engine)),
        Err(NoScores::Unusable)
    );
}

fn control(scores: pi::Knob<pi::ScoreOptions>) -> pi::ChatControl {
    pi::ChatControl {
        tool_choice: pi::ToolChoice::Auto,
        tool_calls: pi::ToolParallelism::One,
        max_output: pi::Knob::Off,
        reasoning: pi::Reasoning::Off,
        sampling: pi::Knob::Off,
        stop: vec![],
        scores,
    }
}

#[test]
fn only_a_choice_of_two_or_more_with_the_knob_set_asks_the_engine() {
    let two = sp::OutputShape::Choice(options(&["a", "b"]));
    let one = sp::OutputShape::Choice(options(&["a"]));
    let on = pi::Knob::Set(pi::ScoreOptions {
        top_k: porter_core::Count(5),
    });
    assert_eq!(
        choice_scores(&control(on), &two),
        sp::ChoiceScores::FirstToken {
            top_k: sp::Count(5)
        }
    );
    assert_eq!(
        choice_scores(&control(pi::Knob::Off), &two),
        sp::ChoiceScores::Off
    );
    assert_eq!(choice_scores(&control(on), &one), sp::ChoiceScores::Off);
    assert_eq!(
        choice_scores(&control(on), &sp::OutputShape::Free),
        sp::ChoiceScores::Off
    );
}

#[test]
fn the_number_of_candidates_asked_for_is_held_to_what_the_engines_take() {
    let two = sp::OutputShape::Choice(options(&["a", "b"]));
    let ask = |k| {
        choice_scores(
            &control(pi::Knob::Set(pi::ScoreOptions {
                top_k: porter_core::Count(k),
            })),
            &two,
        )
    };
    assert_eq!(
        ask(0),
        sp::ChoiceScores::FirstToken {
            top_k: sp::Count(1)
        }
    );
    assert_eq!(
        ask(500),
        sp::ChoiceScores::FirstToken {
            top_k: sp::Count(20)
        }
    );
}

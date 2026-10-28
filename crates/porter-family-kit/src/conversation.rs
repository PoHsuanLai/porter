//! One sign-in conversation for every family: a family writes a [`Flow`] (what it says and asks,
//! and what it does with the answer) and [`Conversation`] is its `SignIn`. The rules every family
//! shared by pasting are written once here: `Cancel` ends the conversation at any step, `Start`
//! begins it (again, after a step back), a review waits for `Confirm` (any other input shows it
//! again), an answer before `Start` cannot be read, and a conversation that has ended answers
//! `Cancelled` to everything but `Start`.

use porter_provider::{SignIn, SignInStep, Signed};
use porter_sheet::{SignInFault, SignInInput};
use std::fmt::Debug;
use std::future::Future;

/// What a family's flow does next.
#[derive(Debug)]
#[non_exhaustive]
pub enum Move<M> {
    /// Say `step` (ask a form, open a browser, wait) and carry on from `M` at the next input.
    Next(SignInStep, M),
    /// Everything is found: show it for the person's review, and wait for `Confirm`.
    Review(Signed),
    /// Finished without a review (the person signed in again).
    Done(Signed),
    /// Ended, for this reason.
    Fail(SignInFault),
}

/// One family's sign-in: the parts that differ. The conversation around it is [`Conversation`].
pub trait Flow: Send {
    /// What the flow keeps between two inputs; `()` when it keeps nothing.
    type Mid: Debug + Send;

    /// Begins, from any state: the first input, and again after a step back.
    fn start(&mut self) -> impl Future<Output = Move<Self::Mid>> + Send;

    /// Takes the answer to the step `mid` was kept for.
    fn resume(
        &mut self,
        mid: Self::Mid,
        input: SignInInput,
    ) -> impl Future<Output = Move<Self::Mid>> + Send;

    /// The person confirmed the review: the result to finish with, or why it cannot be. The
    /// default finishes with what was reviewed.
    fn confirm(
        &mut self,
        signed: Signed,
    ) -> impl Future<Output = Result<Signed, SignInFault>> + Send {
        async move { Ok(signed) }
    }
}

#[derive(Debug)]
enum Phase<M> {
    /// Nothing said yet.
    Fresh,
    /// Waiting for the answer to a step.
    Mid(M),
    /// Shown for review.
    Reviewing(Box<Signed>),
    /// Done, failed or cancelled.
    Ended,
}

/// A [`Flow`] as a `SignIn`.
#[derive(Debug)]
pub struct Conversation<F: Flow> {
    flow: F,
    phase: Phase<F::Mid>,
}

impl<F: Flow> Conversation<F> {
    /// The conversation of `flow`, not yet begun.
    pub fn new(flow: F) -> Self {
        Self {
            flow,
            phase: Phase::Fresh,
        }
    }

    fn settle(&mut self, step: Move<F::Mid>) -> SignInStep {
        match step {
            Move::Next(say, mid) => {
                self.phase = Phase::Mid(mid);
                say
            }
            Move::Review(signed) => {
                let say = review_of(&signed);
                self.phase = Phase::Reviewing(Box::new(signed));
                say
            }
            Move::Done(signed) => SignInStep::Done(signed),
            Move::Fail(fault) => SignInStep::Failed(fault),
        }
    }
}

impl<F: Flow> SignIn for Conversation<F> {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let phase = std::mem::replace(&mut self.phase, Phase::Ended);
        let step = match (phase, input) {
            (_, SignInInput::Cancel) => Move::Fail(SignInFault::Cancelled),
            (_, SignInInput::Start) => self.flow.start().await,
            (Phase::Fresh, _) => Move::Fail(SignInFault::Unreadable),
            (Phase::Mid(mid), input) => self.flow.resume(mid, input).await,
            (Phase::Reviewing(signed), SignInInput::Confirm(_)) => {
                match self.flow.confirm(*signed).await {
                    Ok(signed) => Move::Done(signed),
                    Err(fault) => Move::Fail(fault),
                }
            }
            (Phase::Reviewing(signed), _) => Move::Review(*signed),
            (Phase::Ended, _) => Move::Fail(SignInFault::Cancelled),
        };
        self.settle(step)
    }
}

/// What the review of `signed` shows.
pub(crate) fn review_of(signed: &Signed) -> SignInStep {
    SignInStep::Review {
        claims: signed.claims.clone(),
        endpoints: signed.endpoints.clone(),
        restriction: signed.restriction.clone(),
        label: signed.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AccountLabel, Restriction};

    fn signed() -> Signed {
        Signed::new(
            AccountLabel("ada".into()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Restriction::none(),
        )
    }

    /// Asks once, then reviews.
    #[derive(Debug)]
    struct Script;

    impl Flow for Script {
        type Mid = ();

        async fn start(&mut self) -> Move<()> {
            Move::Next(SignInStep::Waiting, ())
        }

        async fn resume(&mut self, (): (), _input: SignInInput) -> Move<()> {
            Move::Review(signed())
        }
    }

    #[tokio::test]
    async fn a_conversation_asks_reviews_and_finishes_on_confirm() {
        let mut talk = Conversation::new(Script);
        assert_eq!(talk.next(SignInInput::Start).await, SignInStep::Waiting);
        assert!(matches!(
            talk.next(SignInInput::Poll).await,
            SignInStep::Review { .. }
        ));
        assert!(matches!(
            talk.next(SignInInput::Confirm(Vec::new())).await,
            SignInStep::Done(_)
        ));
    }

    #[tokio::test]
    async fn a_review_is_shown_again_until_it_is_confirmed() {
        let mut talk = Conversation::new(Script);
        talk.next(SignInInput::Start).await;
        let first = talk.next(SignInInput::Poll).await;
        assert!(matches!(first, SignInStep::Review { .. }));
        assert_eq!(talk.next(SignInInput::Poll).await, first);
        assert!(matches!(
            talk.next(SignInInput::Confirm(Vec::new())).await,
            SignInStep::Done(_)
        ));
        assert_eq!(
            talk.next(SignInInput::Confirm(Vec::new())).await,
            SignInStep::Failed(SignInFault::Cancelled)
        );
    }

    #[tokio::test]
    async fn cancel_ends_it_at_every_step_and_the_ended_conversation_is_cancelled() {
        for steps in 0..3 {
            let mut talk = Conversation::new(Script);
            let inputs = [
                SignInInput::Start,
                SignInInput::Poll,
                SignInInput::Confirm(Vec::new()),
            ];
            for input in inputs.into_iter().take(steps) {
                talk.next(input).await;
            }
            assert_eq!(
                talk.next(SignInInput::Cancel).await,
                SignInStep::Failed(SignInFault::Cancelled),
                "after {steps} inputs"
            );
            assert_eq!(
                talk.next(SignInInput::Poll).await,
                SignInStep::Failed(SignInFault::Cancelled),
                "after the cancel"
            );
        }
    }

    #[tokio::test]
    async fn an_answer_before_start_is_unreadable_and_start_begins_again() {
        let mut talk = Conversation::new(Script);
        assert_eq!(
            talk.next(SignInInput::Poll).await,
            SignInStep::Failed(SignInFault::Unreadable)
        );
        let mut talk = Conversation::new(Script);
        talk.next(SignInInput::Start).await;
        assert_eq!(talk.next(SignInInput::Start).await, SignInStep::Waiting);
    }
}

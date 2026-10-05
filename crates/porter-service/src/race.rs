//! Two futures, the first one to finish wins and the other is dropped, with no runtime: what
//! the sign-in driver waits on while a poll is in flight and the person may still act.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;

/// Which of two futures finished first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Raced<A, B> {
    /// The first, which is polled first, so it wins a tie.
    First(A),
    /// The second.
    Second(B),
}

/// Polls `first` then `second` until one is ready.
pub(crate) async fn race<A: Future, B: Future>(first: A, second: B) -> Raced<A::Output, B::Output> {
    let (mut first, mut second) = (pin!(first), pin!(second));
    poll_fn(move |cx| match first.as_mut().poll(cx) {
        Poll::Ready(value) => Poll::Ready(Raced::First(value)),
        Poll::Pending => second.as_mut().poll(cx).map(Raced::Second),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};
    use std::task::{Context, Waker};

    fn now<T>(future: impl Future<Output = T>) -> Option<T> {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => Some(value),
            Poll::Pending => None,
        }
    }

    #[test]
    fn the_ready_one_wins_and_the_first_wins_a_tie() {
        assert_eq!(now(race(ready(1), pending::<u8>())), Some(Raced::First(1)));
        assert_eq!(now(race(pending::<u8>(), ready(2))), Some(Raced::Second(2)));
        assert_eq!(now(race(ready(1), ready(2))), Some(Raced::First(1)));
        assert_eq!(now(race(pending::<u8>(), pending::<u8>())), None);
    }
}

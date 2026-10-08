//! A lock that can be held across an await, for the one place the service needs it (saving the
//! registry). porter-service depends on no executor (`scripts/check-boundary.sh` holds it to
//! that), so this is a small first-in-first-out lock over `std` alone, not `tokio::sync::Mutex`.
//! A waiter that is dropped leaves the queue and passes its turn on.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

#[derive(Debug, Default)]
struct State {
    held: bool,
    next: u64,
    waiting: VecDeque<(u64, Waker)>,
}

/// The lock.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    state: Mutex<State>,
}

impl Gate {
    /// Waits for the lock, in the order the callers asked.
    pub(crate) fn lock(&self) -> Acquire<'_> {
        Acquire {
            gate: self,
            ticket: None,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Every critical section is a plain data update that cannot panic midway.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Waiting for the gate.
#[derive(Debug)]
pub(crate) struct Acquire<'a> {
    gate: &'a Gate,
    ticket: Option<u64>,
}

/// The gate, held until this is dropped.
#[derive(Debug)]
pub(crate) struct Held<'a> {
    gate: &'a Gate,
}

impl<'a> Future for Acquire<'a> {
    type Output = Held<'a>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Held<'a>> {
        let this = &mut *self;
        let mut state = this.gate.state();
        // Only the first in line takes it, so a newcomer does not pass those already waiting.
        let first = match this.ticket {
            Some(ticket) => state
                .waiting
                .front()
                .is_some_and(|(front, _)| *front == ticket),
            None => state.waiting.is_empty(),
        };
        if !state.held && first {
            if this.ticket.take().is_some() {
                state.waiting.pop_front();
            }
            state.held = true;
            return Poll::Ready(Held { gate: this.gate });
        }
        let ticket = match this.ticket {
            Some(ticket) => ticket,
            None => {
                let ticket = state.next;
                state.next += 1;
                this.ticket = Some(ticket);
                ticket
            }
        };
        match state.waiting.iter_mut().find(|(t, _)| *t == ticket) {
            Some(entry) => entry.1 = cx.waker().clone(),
            None => state.waiting.push_back((ticket, cx.waker().clone())),
        }
        Poll::Pending
    }
}

impl Drop for Acquire<'_> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            let mut state = self.gate.state();
            state.waiting.retain(|(t, _)| *t != ticket);
            if !state.held
                && let Some((_, next)) = state.waiting.front()
            {
                next.wake_by_ref();
            }
        }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.state();
        state.held = false;
        if let Some((_, next)) = state.waiting.front() {
            next.wake_by_ref();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
        Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn one_holds_the_rest_wait_their_turn_in_order() {
        let gate = Gate::default();
        let mut first = gate.lock();
        let Poll::Ready(held) = poll(&mut first) else {
            panic!("a free gate is taken at once");
        };
        let (mut second, mut third) = (gate.lock(), gate.lock());
        assert!(poll(&mut second).is_pending());
        assert!(poll(&mut third).is_pending());
        drop(held);
        // The third asked later: it does not pass the second.
        assert!(poll(&mut third).is_pending());
        let Poll::Ready(held) = poll(&mut second) else {
            panic!("the second is next");
        };
        assert!(poll(&mut third).is_pending());
        drop(held);
        assert!(poll(&mut third).is_ready());
    }

    #[test]
    fn a_waiter_that_is_dropped_passes_its_turn_on() {
        let gate = Gate::default();
        let Poll::Ready(held) = poll(&mut gate.lock()) else {
            panic!("free");
        };
        let (mut second, mut third) = (gate.lock(), gate.lock());
        assert!(poll(&mut second).is_pending());
        assert!(poll(&mut third).is_pending());
        drop(second);
        drop(held);
        assert!(poll(&mut third).is_ready());
    }
}

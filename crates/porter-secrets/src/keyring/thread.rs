//! Blocking work on a thread of its own, resolved through a oneshot.
//!
//! Attribution: the reason and the shape come from `mail-runtime/src/secrets.rs` in mailo
//! (`off_runtime`, MIT OR Apache-2.0, same author). That one waits on a scoped thread; this
//! crate may not reach an executor, so the wait is a future the thread wakes.

use crate::error::SecretsError;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

/// What the thread leaves for the future.
#[derive(Debug)]
enum Slot<T> {
    Waiting(Option<Waker>),
    Done(T),
    /// The thread ended without a value (it panicked).
    Lost,
}

#[derive(Debug)]
struct Shared<T>(Mutex<Slot<T>>);

impl<T> Shared<T> {
    fn slot(&self) -> MutexGuard<'_, Slot<T>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn settle(&self, now: Slot<T>) {
        let before = std::mem::replace(&mut *self.slot(), now);
        if let Slot::Waiting(Some(waker)) = before {
            waker.wake();
        }
    }
}

/// Settles the slot as lost if dropped before it was given a value.
struct Sender<T>(Arc<Shared<T>>);

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if matches!(&*self.0.slot(), Slot::Waiting(_)) {
            self.0.settle(Slot::Lost);
        }
    }
}

struct Receiver<T>(Arc<Shared<T>>);

impl<T> Future for Receiver<T> {
    type Output = Result<T, SecretsError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut slot = self.0.slot();
        match std::mem::replace(&mut *slot, Slot::Lost) {
            Slot::Done(value) => Poll::Ready(Ok(value)),
            Slot::Lost => Poll::Ready(Err(SecretsError::Unavailable)),
            Slot::Waiting(_) => {
                *slot = Slot::Waiting(Some(cx.waker().clone()));
                Poll::Pending
            }
        }
    }
}

/// Runs `work` on a new thread and resolves with its result. A thread that cannot start or that
/// panics is [`SecretsError::Unavailable`].
pub(super) async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, SecretsError> {
    let shared = Arc::new(Shared(Mutex::new(Slot::Waiting(None))));
    let sender = Sender(shared.clone());
    std::thread::Builder::new()
        .name("porter-keyring".to_owned())
        .spawn(move || {
            let value = work();
            sender.0.settle(Slot::Done(value));
        })
        .map_err(|_| SecretsError::Unavailable)?;
    Receiver(shared).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        struct Park(std::thread::Thread);
        impl std::task::Wake for Park {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Park(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(done) = future.as_mut().poll(&mut cx) {
                return done;
            }
            std::thread::park();
        }
    }

    #[test]
    fn work_resolves_on_another_thread() {
        let here = std::thread::current().id();
        let there = block_on(off_thread(|| std::thread::current().id()));
        assert_ne!(there, Ok(here));
        assert!(there.is_ok());
    }

    #[test]
    fn a_panicking_thread_is_unavailable() {
        let got = block_on(off_thread(|| -> u8 { panic!("the keyring crashed") }));
        assert_eq!(got, Err(SecretsError::Unavailable));
    }
}

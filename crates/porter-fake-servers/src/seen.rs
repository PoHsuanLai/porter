//! What a fake received, kept for the test to assert on, and the handle that runs a fake.

use porter_fake::FakeServer;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::task::JoinHandle;

/// An append-only record shared between a fake's tasks and the test holding its handle.
#[derive(Debug)]
pub struct Seen<T>(Arc<Mutex<Vec<T>>>);

impl<T> Clone for Seen<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for Seen<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Clone> Seen<T> {
    /// Records one more.
    pub fn push(&self, item: T) {
        lock(&self.0).push(item);
    }

    /// Everything so far, oldest first.
    pub fn all(&self) -> Vec<T> {
        lock(&self.0).clone()
    }

    /// Everything so far that `keep` admits.
    pub fn matching(&self, keep: impl Fn(&T) -> bool) -> Vec<T> {
        lock(&self.0).iter().filter(|t| keep(t)).cloned().collect()
    }
}

/// A lock that survives a panicking task (a failed assertion elsewhere).
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fake serving on a task; dropping it stops the server. It derefs to the fake's handle,
/// which holds the recordings and the knobs.
#[derive(Debug)]
pub struct Running<H> {
    handle: H,
    task: JoinHandle<()>,
}

impl<H> Running<H> {
    /// Runs `server` until this is dropped.
    pub fn spawn<S: FakeServer + 'static>(server: S, handle: H) -> Self {
        Self {
            handle,
            task: tokio::spawn(server.serve()),
        }
    }
}

impl<H> Deref for Running<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.handle
    }
}

impl<H> Drop for Running<H> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

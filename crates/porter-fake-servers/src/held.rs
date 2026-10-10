//! A runtime that runs nothing until a test lets it: the seam for "a call made as soon as the
//! name is owned is answered". zbus starts a connection's object server on a task of its own at
//! its first use, and a call that arrives before that task listens is dropped. A test enters the
//! held runtime while it makes the connection's object server, so that task cannot run. A daemon
//! that claims its name before its calls are taken is then serving with nobody listening, and
//! the call the test sends is lost; a daemon that claims its name only once its calls are taken
//! cannot finish starting until the runtime is released.
//!
//! No timer holds the runtime for long: [`HeldRuntime::start`] gives the start a short pause to
//! show which of the two it is, and releases the runtime if the start has not finished by then.
//! A pause that a loaded machine stretches only lets a bad daemon pass by luck (the test gets
//! weaker, never flaky). The runtime is also released when the [`HeldRuntime`] is dropped, so a
//! test that panics does not leave the thread waiting.

use std::future::Future;
use std::pin::pin;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;

/// A current-thread runtime whose thread runs nothing until it is released.
#[derive(Debug)]
pub struct HeldRuntime {
    handle: Handle,
    release: Option<mpsc::Sender<()>>,
    stop: Option<oneshot::Sender<()>>,
    runner: Option<JoinHandle<()>>,
}

impl HeldRuntime {
    /// Builds the runtime and its thread, which waits to be released.
    ///
    /// # Panics
    /// The runtime cannot be built.
    #[must_use]
    pub fn new() -> Self {
        let held = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let handle = held.handle().clone();
        let (release, released) = mpsc::channel::<()>();
        let (stop, stopped) = oneshot::channel::<()>();
        let runner = std::thread::spawn(move || {
            // Returns when released, or when the sender is dropped.
            let _ = released.recv();
            held.block_on(async {
                let _ = stopped.await;
            });
        });
        Self {
            handle,
            release: Some(release),
            stop: Some(stop),
            runner: Some(runner),
        }
    }

    /// The runtime's handle: `enter()` it while making the object server.
    #[must_use]
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Drives `starting` (the daemon's start, which claims its name) for `pause`. If it has not
    /// finished by then it is waiting for the held runtime, so the runtime is released and the
    /// start driven to its end (at most [`porter_fake::GENEROUS`]). Returns its output.
    ///
    /// # Panics
    /// The start did not finish within [`porter_fake::GENEROUS`] of the release.
    pub async fn start<F: Future>(&mut self, starting: F, pause: Duration) -> F::Output {
        let mut starting = pin!(starting);
        if let Ok(done) = tokio::time::timeout(pause, &mut starting).await {
            return done;
        }
        self.release();
        tokio::time::timeout(porter_fake::GENEROUS, starting)
            .await
            .expect("the start finishes once the runtime runs")
    }

    /// Lets the runtime run once the test's call is on its way: if the runtime is still held
    /// (the start finished without it, so the daemon is serving with nobody listening), waits
    /// `pause` for the call to leave and then releases it. Does nothing if it ran already.
    pub async fn release_once_sent(&mut self, pause: Duration) {
        if self.release.is_some() {
            tokio::time::sleep(pause).await;
            self.release();
        }
    }

    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }

    /// Releases the runtime if it is not yet, stops it and joins its thread.
    ///
    /// # Panics
    /// The runtime's thread panicked.
    pub fn finish(mut self) {
        self.release();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(runner) = self.runner.take() {
            runner.join().expect("the held runtime's thread");
        }
    }
}

impl Default for HeldRuntime {
    fn default() -> Self {
        Self::new()
    }
}

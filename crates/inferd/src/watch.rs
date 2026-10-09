//! The loop that keeps the probed runtimes current: at start, on `Rescan`, and on a timer whose
//! wait doubles while nothing changes (to the longest the `[probe]` table allows) and starts over
//! when something does.
//!
//! One look ([`Watch::look`]) probes every configured port, compares what answered with what the
//! daemon last installed ([`plan`], pure), installs the change in the engines' probed book, and
//! reports it to accountd. A runtime that answers is `Online` and reported `ok` with its models;
//! one that stops is `Offline` and reported `offline`, and its models stay listed with nothing to
//! serve them. A report accountd did not take is asked again at the next look; the models are
//! served either way (a local model needs no grant), under the account id the provider's id makes.
//!
//! A runtime that is slow is not a runtime that stopped. When a look finds an online runtime gone
//! and some request of that look timed out (a loaded computer answers late), the runtime is kept
//! as it was and looked at again soon; only [`MISSES_TO_DOWN`] such looks in a row mark it
//! offline. A refused connection (nothing listens) is a stop at once.

use crate::engines::Engines;
use crate::probe::{Found, ProbeConfig, Runtime, probe};
use crate::probed::Standing;
use crate::report::{ReportFault, Reports};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Whether accountd has been told of what is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    /// It has.
    Yes,
    /// It has not (it was not reachable): the next look tells it.
    No,
}

/// What the daemon last installed for one runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Whether the runtime answered.
    pub standing: Standing,
    /// What it served the last time it answered.
    pub found: Option<Found>,
    /// Whether accountd knows.
    pub told: Told,
}

/// One change a look makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A runtime answered, and it is not what is installed: install it and report it `ok`.
    Up(Found),
    /// A runtime that was online did not answer: mark it offline and report that.
    Down(Runtime),
}

/// What `now` (the runtimes that answered) changes of `installed`.
pub fn plan(installed: &BTreeMap<Runtime, Installed>, now: &[Found]) -> Vec<Change> {
    Runtime::ALL
        .into_iter()
        .filter_map(|runtime| {
            let have = installed.get(&runtime);
            match now.iter().find(|found| found.runtime == runtime) {
                Some(found) => {
                    let same = have.is_some_and(|one| {
                        one.standing == Standing::Online && one.found.as_ref() == Some(found)
                    });
                    (!same).then(|| Change::Up(found.clone()))
                }
                None => have
                    .is_some_and(|one| one.standing == Standing::Online)
                    .then_some(Change::Down(runtime)),
            }
        })
        .collect()
}

/// How many looks in a row must lose a runtime to a timeout before it is marked offline.
pub const MISSES_TO_DOWN: u32 = 3;

/// The wait before looking again at a runtime a slow look could not find (never over `base`).
const DOUBT_WAIT: Duration = Duration::from_secs(2);

/// Holds back the `Down` changes of a look in which a request timed out, until a runtime has
/// been lost to timeouts [`MISSES_TO_DOWN`] looks in a row. `misses` is each runtime's count so
/// far; the runtimes that answered start over. Whether any change was held back.
pub fn confirm(
    misses: &mut BTreeMap<Runtime, u32>,
    changes: Vec<Change>,
    answered: &[Found],
    timed_out: bool,
) -> (Vec<Change>, bool) {
    misses.retain(|runtime, _| answered.iter().all(|found| found.runtime != *runtime));
    let mut held = false;
    let kept = changes
        .into_iter()
        .filter(|change| match change {
            Change::Up(_) => true,
            Change::Down(runtime) if !timed_out => {
                misses.remove(runtime);
                true
            }
            Change::Down(runtime) => {
                let count = misses.entry(*runtime).or_insert(0);
                *count += 1;
                let down = *count >= MISSES_TO_DOWN;
                if down {
                    misses.remove(runtime);
                }
                held |= !down;
                down
            }
        })
        .collect();
    (kept, held)
}

/// An [`Http`] that notes whether any request through it timed out.
struct Noting<'a, H> {
    inner: &'a H,
    timed_out: AtomicBool,
}

impl<H: Http> Http for Noting<'_, H> {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let sent = self.inner.send(request).await;
        if matches!(sent, Err(HttpError::TimedOut)) {
            self.timed_out.store(true, Ordering::Relaxed);
        }
        sent
    }
}

/// The wait after a look: `base` when something changed, else double `current`, up to `longest`.
pub fn next_wait(current: Duration, changed: bool, base: Duration, longest: Duration) -> Duration {
    match changed {
        true => base,
        false => current.saturating_mul(2).clamp(base, longest),
    }
}

/// What a look is made of.
#[derive(Debug)]
pub struct Watch<H> {
    http: H,
    config: ProbeConfig,
    engines: Engines,
    reports: Arc<dyn Reports>,
    sockets: PathBuf,
    installed: BTreeMap<Runtime, Installed>,
    misses: BTreeMap<Runtime, u32>,
    doubtful: bool,
}

impl<H: Http + 'static> Watch<H> {
    /// A watch over the ports `config` names, installing into `engines` and reporting to
    /// `reports`. `sockets` is where an engine's socket would go (unused by a probed model).
    pub fn new(
        http: H,
        config: ProbeConfig,
        engines: Engines,
        reports: Arc<dyn Reports>,
        sockets: PathBuf,
    ) -> Self {
        Self {
            http,
            config,
            engines,
            reports,
            sockets,
            installed: BTreeMap::new(),
            misses: BTreeMap::new(),
            doubtful: false,
        }
    }

    /// One look: probe, install the changes, report what accountd has not been told. Whether
    /// anything changed.
    pub async fn look(&mut self) -> bool {
        let noting = Noting {
            inner: &self.http,
            timed_out: AtomicBool::new(false),
        };
        let found = probe(&noting, &self.config).await;
        let timed_out = noting.timed_out.load(Ordering::Relaxed);
        let (changes, held) = confirm(
            &mut self.misses,
            plan(&self.installed, &found),
            &found,
            timed_out,
        );
        self.doubtful = held;
        let changed = !changes.is_empty();
        for change in changes {
            match change {
                Change::Up(found) => self.up(found),
                Change::Down(runtime) => self.down(runtime),
            }
        }
        for runtime in Runtime::ALL {
            self.tell(runtime).await;
        }
        changed
    }

    fn up(&mut self, found: Found) {
        let models = found.local_models(&self.sockets);
        self.engines
            .probed()
            .set(found.runtime, Standing::Online, Some(models));
        self.installed.insert(
            found.runtime,
            Installed {
                standing: Standing::Online,
                found: Some(found),
                told: Told::No,
            },
        );
    }

    fn down(&mut self, runtime: Runtime) {
        self.engines.probed().set(runtime, Standing::Offline, None);
        if let Some(one) = self.installed.get_mut(&runtime) {
            one.standing = Standing::Offline;
            one.told = Told::No;
        }
    }

    /// Tells accountd of `runtime` if it has not been told.
    async fn tell(&mut self, runtime: Runtime) {
        let Some(one) = self.installed.get(&runtime) else {
            return;
        };
        if one.told == Told::Yes {
            return;
        }
        let claims = match one.standing {
            Standing::Online => one.found.as_ref().map(Found::claims).unwrap_or_default(),
            Standing::Offline => Vec::new(),
        };
        let standing = one.standing;
        let reported = self.reports.report(runtime, &claims, standing).await;
        // A rejection is final for this content (asking again would say the same); only a
        // report that did not get through is asked again.
        let done = !matches!(reported, Err(ReportFault::Unreachable));
        if let (true, Some(one)) = (done, self.installed.get_mut(&runtime)) {
            one.told = Told::Yes;
        }
    }

    /// Runs the loop on a task: a look at once, then on every `now()` and when the wait is up.
    pub fn spawn(mut self) -> Probing {
        let (asks, mut inbox) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        let (base, longest) = (self.config.base(), self.config.longest());
        let task = tokio::spawn(async move {
            let mut wait = Duration::ZERO;
            loop {
                let asked = tokio::select! {
                    () = tokio::time::sleep(wait) => None,
                    ask = inbox.recv() => match ask {
                        Some(ask) => Some(ask),
                        None => return,
                    },
                };
                let changed = self.look().await;
                if let Some(ask) = asked {
                    let _ = ask.send(());
                }
                wait = match (self.doubtful, wait.is_zero()) {
                    (true, _) => DOUBT_WAIT.min(base),
                    (false, true) => base,
                    (false, false) => next_wait(wait, changed, base, longest),
                };
            }
        });
        Probing {
            inner: Arc::new(Running { asks, task }),
        }
    }
}

struct Running {
    asks: mpsc::UnboundedSender<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The running loop. Dropping the last clone stops it.
#[derive(Clone)]
pub struct Probing {
    inner: Arc<Running>,
}

impl std::fmt::Debug for Probing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probing").finish_non_exhaustive()
    }
}

impl Probing {
    /// Looks now and answers when the look is done (`Inference1.Rescan`).
    pub async fn now(&self) {
        let (done, wait) = oneshot::channel();
        if self.inner.asks.send(done).is_ok() {
            let _ = wait.await;
        }
    }
}

#[cfg(test)]
mod tests;

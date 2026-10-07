//! The agent launchers (lane agent-login; design/31 R7-R9, C5, C6): which bus connection launches
//! which agent program, and the requests it has been asked to carry out.
//!
//! Only the agent can log itself in, so accountd never holds, sees or relays the login: it tells
//! the registered launcher "sign this agent in" (or out) in a signal sent to that connection
//! alone, and takes back a coarse outcome (`LoginOutcome`, a closed set of words). Nothing else
//! is on the bus: no token, no URL, no code.
//!
//! - A registration lives as long as the registering connection (`NameOwnerChanged`, see
//!   `Core::left`). One launcher per program, first wins; a program held by a connection that is
//!   no longer on the bus is free even before the departure has been seen.
//! - A request is pending until the launcher reports it, the bound runs out on accountd's own
//!   clock (`LoginTiming`), or the launcher leaves the bus. Whoever waits is told how it ended
//!   ([`LoginEnd`]); a request outlives its waiter (a sheet closed by the person does not
//!   withdraw it).

use crate::errors::RefusedError;
use porter_core::capability::AgentProgram;
use porter_core::{AccountId, LoginRequestId, UnixSeconds};
use porter_dbus::{ACCOUNTS_PATH, LauncherFault};
use porter_service::{Clock, Launchers as LaunchersSeam, LoginEnd, NoLauncher, Waiting};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::oneshot;
use zbus::Connection;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

/// The interface the two signals are on.
const PEER_INTERFACE: &str = "org.quire.Accounts1.Peer";

/// How long a request waits for the launcher: ten minutes, since a person may be finishing a
/// login in a browser.
pub const DEFAULT_BOUND: Duration = Duration::from_secs(600);

/// How often a pending request's bound is looked at.
pub const DEFAULT_TICK: Duration = Duration::from_secs(1);

/// What a launcher is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    Login,
    Logout,
}

impl Ask {
    fn signal(self) -> &'static str {
        match self {
            Ask::Login => "AgentLoginRequested",
            Ask::Logout => "AgentLogoutRequested",
        }
    }
}

/// The time seam of the requests: accountd's own clock, the bound, and how often the bound is
/// looked at. The default is the system clock, ten minutes and one second.
#[derive(Clone)]
pub struct LoginTiming {
    /// The clock the bound is measured on; none is the system's.
    pub clock: Option<Arc<dyn Clock>>,
    /// How long a request waits for its report.
    pub bound: Duration,
    /// How often a pending request is checked against the bound.
    pub tick: Duration,
}

impl Default for LoginTiming {
    fn default() -> Self {
        Self {
            clock: None,
            bound: DEFAULT_BOUND,
            tick: DEFAULT_TICK,
        }
    }
}

impl std::fmt::Debug for LoginTiming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginTiming")
            .field("clock", &self.clock.as_ref().map(|_| "<injected>"))
            .field("bound", &self.bound)
            .field("tick", &self.tick)
            .finish()
    }
}

/// A request waiting for its report.
#[derive(Debug)]
pub(crate) struct Pending {
    owner: String,
    ask: Ask,
    pub(crate) account: AccountId,
    deadline: UnixSeconds,
    done: oneshot::Sender<LoginEnd>,
}

impl Pending {
    /// Tells whoever waits how it ended.
    pub(crate) fn end(self, end: LoginEnd) {
        let _ = self.done.send(end);
    }
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    /// The connection (unique name) that launches each program.
    programs: BTreeMap<AgentProgram, String>,
    pending: BTreeMap<LoginRequestId, Pending>,
}

/// The registered launchers and their pending requests.
#[derive(Debug)]
pub(crate) struct Launchers {
    state: Arc<Mutex<State>>,
    connection: Connection,
    timing: LoginTiming,
}

fn held(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    // Every critical section is a plain data update.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The system clock, for a `LoginTiming` that names none.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        UnixSeconds(i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
    }
}

impl Launchers {
    pub(crate) fn new(connection: Connection, timing: LoginTiming) -> Self {
        Self {
            state: Arc::default(),
            connection,
            timing,
        }
    }

    fn clock(&self) -> Arc<dyn Clock> {
        self.timing
            .clock
            .clone()
            .unwrap_or_else(|| Arc::new(SystemClock))
    }

    /// Whether `name` is a connection on the bus now.
    async fn on_bus(&self, name: &str) -> bool {
        let Ok(bus) = DBusProxy::new(&self.connection).await else {
            return false;
        };
        match BusName::try_from(name.to_owned()) {
            Ok(name) => bus.name_has_owner(name).await.unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Makes `owner` the launcher of `programs`. All or none: a program a live connection other
    /// than `owner` holds is `AlreadyRegistered` and nothing is taken. A program `owner` holds
    /// already stays as it is.
    pub(crate) async fn register(
        &self,
        owner: &str,
        programs: &[AgentProgram],
    ) -> Result<(), LauncherFault> {
        let rivals: Vec<String> = {
            let state = held(&self.state);
            programs
                .iter()
                .filter_map(|program| state.programs.get(program))
                .filter(|holder| holder.as_str() != owner)
                .cloned()
                .collect()
        };
        let mut gone = Vec::new();
        for rival in rivals {
            if !self.on_bus(&rival).await {
                gone.push(rival);
            }
        }
        let mut state = held(&self.state);
        let taken = programs.iter().any(|program| {
            state
                .programs
                .get(program)
                .is_some_and(|holder| holder != owner && !gone.contains(holder))
        });
        if taken {
            return Err(LauncherFault::AlreadyRegistered);
        }
        for program in programs {
            state.programs.insert(program.clone(), owner.to_owned());
        }
        Ok(())
    }

    /// A connection left the bus: its programs are free and its pending requests end.
    pub(crate) fn left(&self, name: &str) {
        let ended: Vec<Pending> = {
            let mut state = held(&self.state);
            state.programs.retain(|_, holder| holder != name);
            let ids: Vec<LoginRequestId> = state
                .pending
                .iter()
                .filter(|(_, pending)| pending.owner == name)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| state.pending.remove(id))
                .collect()
        };
        for pending in ended {
            pending.end(LoginEnd::LauncherGone);
        }
    }

    /// Takes the request `request` of `kind` away, for the connection `owner` that was sent it:
    /// any other connection, kind or id is `UnknownRequest`.
    pub(crate) fn take(
        &self,
        owner: &str,
        request: &LoginRequestId,
        kind: Ask,
    ) -> Result<Pending, RefusedError> {
        let mut state = held(&self.state);
        match state.pending.get(request) {
            Some(pending) if pending.owner == owner && pending.ask == kind => {
                state.pending.remove(request).ok_or_else(unknown)
            }
            _ => Err(unknown()),
        }
    }

    async fn ask(
        &self,
        kind: Ask,
        account: &AccountId,
        program: &AgentProgram,
    ) -> Result<Waiting, NoLauncher> {
        let clock = self.clock();
        let (request, owner, waiting) = {
            let mut state = held(&self.state);
            let owner = state.programs.get(program).cloned().ok_or(NoLauncher)?;
            state.next += 1;
            let request =
                LoginRequestId::parse(&format!("login-{}", state.next)).map_err(|_| NoLauncher)?;
            let (done, waiting) = oneshot::channel();
            let bound = i64::try_from(self.timing.bound.as_secs()).unwrap_or(i64::MAX);
            state.pending.insert(
                request.clone(),
                Pending {
                    owner: owner.clone(),
                    ask: kind,
                    account: account.clone(),
                    deadline: UnixSeconds(clock.now().0.saturating_add(bound)),
                    done,
                },
            );
            (request, owner, waiting)
        };
        tokio::spawn(expire(
            Arc::clone(&self.state),
            request.clone(),
            clock,
            self.timing.tick,
        ));
        let sent = self
            .send(kind, &owner, &request, account, program)
            .await
            .is_ok();
        if !sent {
            held(&self.state).pending.remove(&request);
            return Err(NoLauncher);
        }
        Ok(Box::pin(async move {
            waiting.await.unwrap_or(LoginEnd::LauncherGone)
        }))
    }

    /// The signal, to `owner` alone.
    async fn send(
        &self,
        kind: Ask,
        owner: &str,
        request: &LoginRequestId,
        account: &AccountId,
        program: &AgentProgram,
    ) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(&self.connection, ACCOUNTS_PATH)?
            .set_destination(BusName::try_from(owner.to_owned())?);
        emitter
            .emit(
                PEER_INTERFACE,
                kind.signal(),
                &(request.as_str(), account.as_str(), program.as_str()),
            )
            .await
    }
}

fn unknown() -> RefusedError {
    RefusedError::launcher(
        LauncherFault::UnknownRequest,
        "no such request for this connection",
    )
}

/// Ends the request when its bound runs out on `clock`; ends itself when the request is gone.
async fn expire(
    state: Arc<Mutex<State>>,
    request: LoginRequestId,
    clock: Arc<dyn Clock>,
    tick: Duration,
) {
    loop {
        tokio::time::sleep(tick).await;
        let mut guard = held(&state);
        match guard.pending.get(&request) {
            None => return,
            Some(pending) if clock.now() >= pending.deadline => {
                let pending = guard.pending.remove(&request);
                drop(guard);
                if let Some(pending) = pending {
                    pending.end(LoginEnd::Expired);
                }
                return;
            }
            Some(_) => {}
        }
    }
}

impl LaunchersSeam for Launchers {
    async fn ask_login(
        &self,
        account: &AccountId,
        program: &AgentProgram,
    ) -> Result<Waiting, NoLauncher> {
        self.ask(Ask::Login, account, program).await
    }

    async fn ask_logout(
        &self,
        account: &AccountId,
        program: &AgentProgram,
    ) -> Result<Waiting, NoLauncher> {
        self.ask(Ask::Logout, account, program).await
    }
}

/// The word the Settings pane hears on `accounts.<id>.sign_out` once a sign out has an outcome
/// (`Changed(key, word)`); the action itself returns when the request is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignOutNews {
    /// The launcher was asked; the outcome follows.
    Asked,
    /// No launcher is registered: porter forgot the sign-in and the agent's own login was not
    /// touched.
    LoginUntouched,
    /// The agent says it is signed out.
    Done,
    /// The launcher reported a failure.
    Failed,
    /// The launcher or the person gave it up.
    Cancelled,
    /// The launcher did not report within the bound.
    Expired,
    /// The launcher left the bus.
    LauncherGone,
}

impl SignOutNews {
    pub(crate) fn slug(self) -> &'static str {
        match self {
            SignOutNews::Asked => "asked",
            SignOutNews::LoginUntouched => "login_untouched",
            SignOutNews::Done => "done",
            SignOutNews::Failed => "failed",
            SignOutNews::Cancelled => "cancelled",
            SignOutNews::Expired => "expired",
            SignOutNews::LauncherGone => "launcher_gone",
        }
    }

    /// What an end of a logout request is to the pane.
    pub(crate) fn of(end: LoginEnd) -> Self {
        use porter_core::LoginOutcome;
        match end {
            LoginEnd::Reported(LoginOutcome::Ready) => SignOutNews::Done,
            LoginEnd::Reported(LoginOutcome::Failed(_)) => SignOutNews::Failed,
            LoginEnd::Reported(LoginOutcome::Cancelled) => SignOutNews::Cancelled,
            LoginEnd::Expired => SignOutNews::Expired,
            LoginEnd::LauncherGone => SignOutNews::LauncherGone,
        }
    }
}

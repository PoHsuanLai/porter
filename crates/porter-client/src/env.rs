//! What `Accounts::connect` may reach, injected by the app (CONVENTIONS §6: nothing ambient).

use std::path::PathBuf;
use std::time::Duration;

/// How long a client waits for an agent it just started (process startup, not the agent's first
/// piece of work: the agent takes its lock and opens its door first). Thirty seconds: an idle
/// computer starts the agent in well under a second, but a loaded one (a big build, swap in use)
/// can take several seconds to exec and load it, and a client that gives up first fails a person
/// for the machine's sake. A client that gets its answer returns at once, so the wait costs only
/// a start that really failed.
pub const START_WAIT: Duration = Duration::from_secs(30);

/// Whether, and how, a client starts the agent when nobody answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartAgent {
    /// Never: an agent that is not running is `Unreachable` (the default; accountd is started by
    /// the session, or by the app that hosts it).
    #[default]
    Never,
    /// Launch this same binary, detached, with `args` (`latchkey::spawn`), and wait up to `wait`
    /// for it to answer. latchkey's lock decides whether it is the one: a client that races
    /// another starts a second process that finds the lock taken and exits.
    Spawn {
        /// The arguments of the agent subcommand of this binary.
        args: Vec<String>,
        /// How long to wait for the agent to answer.
        wait: Duration,
    },
}

/// Where the runtime directory is, for an agent that is not in the process environment's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Place {
    /// latchkey's own rules over the process environment (`XDG_RUNTIME_DIR` on Linux, `TMPDIR` on
    /// macOS, a pipe named for the user on Windows): the client and the agent it starts agree.
    #[default]
    Ambient,
    /// Under this runtime directory instead (a sandbox, a test, an app that keeps its own).
    RuntimeDir(PathBuf),
}

/// The per-user agent on the latchkey socket: its name, where latchkey puts it, and whether a
/// client starts it. The address is latchkey's (`<runtime>/<name>/agent.sock`), never a path the
/// app writes down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketAgent {
    /// The agent's name (ASCII letters, digits and `-`: it becomes a directory).
    pub name: String,
    /// Where its runtime directory is.
    pub place: Place,
    /// Whether a client starts it.
    pub start: StartAgent,
}

impl SocketAgent {
    /// The name porter's own agent has.
    pub const PORTER: &'static str = "porter";

    /// The agent called `name` by latchkey's rules over the process environment, never started
    /// by the client.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            place: Place::Ambient,
            start: StartAgent::Never,
        }
    }

    /// Porter's own agent (`porter`).
    pub fn porter() -> Self {
        Self::named(Self::PORTER)
    }

    /// The same agent under `dir` as its runtime directory.
    pub fn in_runtime_dir(self, dir: PathBuf) -> Self {
        Self {
            place: Place::RuntimeDir(dir),
            ..self
        }
    }

    /// The same agent, started by the client with `start` when nobody answers.
    pub fn starting(self, start: StartAgent) -> Self {
        Self { start, ..self }
    }
}

/// A daemon link to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkChoice {
    /// accountd on the session bus.
    Dbus,
    /// accountd (or an app-hosted agent) on the latchkey socket.
    Socket(SocketAgent),
}

/// The links to try, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEnv {
    /// The links, most preferred first.
    pub links: Vec<LinkChoice>,
}

//! The socket carrier over a Windows named pipe, at the name latchkey gives the agent
//! (`<agent>-<user>`, in the machine-wide pipe namespace: `\\.\pipe\<name>`).
//!
//! latchkey decides the name, whether an agent already holds it (an advisory lock) and how one is
//! started; the bytes go over tokio's own pipe client, since latchkey's `Stream` is a blocking one.
//! A pipe serves one client per instance and refuses the rest with `ERROR_PIPE_BUSY` (where a Unix
//! socket queues them), so a knock that finds it busy tries again for a bounded time before it
//! gives up as `Unreachable`.
//!
//! No descriptors cross a pipe (Windows has no `SCM_RIGHTS`), so what rides on one on Unix is
//! refused here:
//!
//! - `open_authenticated` and `open_linked` (a relay's stream arrives as a descriptor on the
//!   reply) answer `TransportError::Unreachable`, the contract of a transport that cannot carry
//!   one; an app on Windows keeps the relay in process (`InProcess` with a `RelayHost`);
//! - a session's frame that names attached images (`ImageSource::Attached`) is a
//!   `SessionError::Malformed`; images go inline (`ImageSource::Inline`).
//!
//! Not built or run on the machines this was written on: it is checked for
//! `x86_64-pc-windows-msvc` from Linux, never executed.

use crate::authenticated::Relayed;
use crate::env::{Place, SocketAgent, StartAgent};
use crate::error::TransportError;
use crate::transport::pipe::PipeSession;
use latchkey::{Agent, Endpoint, Environment, Error, here};
use porter_core::{AccountsReply, AccountsRequest};
use std::time::{Duration, Instant};
use tokio::net::windows::named_pipe::ClientOptions;

/// The session's link: frames over the connected pipe.
#[cfg(feature = "infer")]
pub(super) type Link = PipeSession;

/// `ERROR_PIPE_BUSY`: every instance of the pipe is in use.
const ERROR_PIPE_BUSY: i32 = 231;

/// How long a knock keeps trying a busy pipe, and how often (latchkey's own knock is two
/// seconds).
const BUSY_FOR: Duration = Duration::from_secs(2);
const BUSY_EVERY: Duration = Duration::from_millis(50);

/// The agent's address as latchkey resolved it, or why it could not be.
#[derive(Debug)]
pub(super) struct Door(Result<Agent, TransportError>);

/// latchkey's address for `agent`: its own rules over the process environment, or under the
/// runtime directory the app named.
pub(super) fn door(agent: &SocketAgent) -> Door {
    let built = match &agent.place {
        Place::Ambient => Agent::new(&agent.name),
        Place::RuntimeDir(dir) => {
            let env = Environment {
                runtime_dir: Some(dir.as_os_str()),
                tmpdir: Some(dir.as_os_str()),
                local_app_data: Some(dir.as_os_str()),
                ..Environment::default()
            };
            Agent::in_environment(&agent.name, here(), &env)
        }
    };
    Door(built.map_err(|error| match error {
        Error::Homeless(_) => TransportError::Unreachable,
        other => TransportError::Malformed(format!("agent address: {other}")),
    }))
}

impl Door {
    fn agent(&self) -> Result<&Agent, TransportError> {
        self.0.as_ref().map_err(Clone::clone)
    }

    /// `\\.\pipe\<name>`.
    fn pipe(&self) -> Result<String, TransportError> {
        match &self.agent()?.address().endpoint {
            Endpoint::Pipe(name) => Ok(format!(r"\\.\pipe\{name}")),
            Endpoint::Socket(_) => Err(TransportError::Unreachable),
        }
    }
}

/// A connect that failed: nobody there is `Unreachable`, anything else is the pipe misbehaving.
fn unreachable_or_malformed(error: &std::io::Error) -> TransportError {
    match error.kind() {
        std::io::ErrorKind::NotFound
        | std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::BrokenPipe => TransportError::Unreachable,
        _ => TransportError::Malformed(format!("pipe: {error}")),
    }
}

async fn knock(door: &Door) -> Result<PipeSession, TransportError> {
    let name = door.pipe()?;
    let began = Instant::now();
    loop {
        match ClientOptions::new().open(&name) {
            Ok(pipe) => return Ok(PipeSession::from_pipe(pipe)),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if began.elapsed() >= BUSY_FOR {
                    return Err(TransportError::Unreachable);
                }
                tokio::time::sleep(BUSY_EVERY).await;
            }
            Err(error) => return Err(unreachable_or_malformed(&error)),
        }
    }
}

/// Starts the agent by the app's policy, off the runtime (latchkey's wait is a blocking one).
async fn start(door: &Door, how: &StartAgent) -> Result<(), TransportError> {
    let StartAgent::Spawn { args, wait } = how else {
        return Err(TransportError::Unreachable);
    };
    let agent = door.agent()?.clone();
    let (args, wait) = (args.clone(), *wait);
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        agent
            .connect_or_start(|| latchkey::spawn(&args), wait)
            .map(drop)
    })
    .await
    .map_err(|_| TransportError::Closed)?
    .map_err(|error| match error {
        Error::NeverAnswered(_) | Error::CannotSpawn(_) | Error::NoSelf(_) | Error::Busy => {
            TransportError::Unreachable
        }
        other => TransportError::Malformed(format!("agent: {other}")),
    })
}

/// A connection to the agent: knock, and start it if nobody answers and the app said to.
async fn connect(door: &Door, how: &StartAgent) -> Result<PipeSession, TransportError> {
    match knock(door).await {
        Err(TransportError::Unreachable) if *how != StartAgent::Never => {
            start(door, how).await?;
            knock(door).await
        }
        reached => reached,
    }
}

pub(super) async fn reachable(door: &Door, how: &StartAgent) -> bool {
    connect(door, how).await.is_ok()
}

/// One call: a request frame out, a reply frame back.
pub(super) async fn call(
    door: &Door,
    how: &StartAgent,
    request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    let mut link = connect(door, how).await?;
    let sent = link.write_body(&request).await;
    match link.read_body::<AccountsReply>().await {
        Ok(reply) => Ok(reply),
        Err(read) => Err(sent.err().unwrap_or(read)),
    }
}

/// A session: the hello out, then the link is the caller's.
#[cfg(feature = "infer")]
pub(super) async fn open(
    door: &Door,
    how: &StartAgent,
    hello: porter_infer::LinkHello,
) -> Result<Link, TransportError> {
    let mut link = connect(door, how).await?;
    link.write_body(&hello).await?;
    Ok(link)
}

/// A relay's stream is a descriptor riding on the reply, and a pipe carries none: refused as a
/// transport that cannot carry one is.
pub(super) async fn open_authenticated(
    _door: &Door,
    _how: &StartAgent,
    _request: AccountsRequest,
) -> Result<Relayed, TransportError> {
    Err(TransportError::Unreachable)
}

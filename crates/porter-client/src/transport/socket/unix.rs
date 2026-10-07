//! The socket carrier over a Unix stream socket, at the address latchkey gives the agent.
//!
//! latchkey decides where the socket is, whether an agent already holds it (an advisory lock, not
//! a look at the file), how one is knocked on and how one is started. The connected stream comes
//! out of latchkey as a descriptor (`latchkey::into_fd`) and becomes a tokio stream, so the bytes
//! go over a plain Unix socket and a relay's descriptor can ride on the reply as SCM_RIGHTS.

use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::env::{Place, SocketAgent, StartAgent};
use crate::error::TransportError;
use crate::transport::framed::FramedSession;
use latchkey::{Agent, Environment, Error, here};
use porter_core::{AccountsReply, AccountsRequest};
use std::io::ErrorKind;
use std::os::unix::net::UnixStream as StdStream;
use tokio::net::UnixStream;

/// The session's link: frames over the connected stream.
#[cfg(feature = "infer")]
pub(super) type Link = FramedSession;

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
                ..Environment::default()
            };
            Agent::in_environment(&agent.name, here(), &env)
        }
    };
    Door(built.map_err(|error| match error {
        // Nowhere to put a socket is nobody to talk to.
        Error::Homeless(_) => TransportError::Unreachable,
        other => TransportError::Malformed(format!("agent address: {other}")),
    }))
}

impl Door {
    fn agent(&self) -> Result<&Agent, TransportError> {
        self.0.as_ref().map_err(Clone::clone)
    }
}

/// A connect that failed: nobody listening is `Unreachable`, anything else is the socket
/// misbehaving.
fn unreachable_or_malformed(error: &std::io::Error) -> TransportError {
    match error.kind() {
        ErrorKind::NotFound
        | ErrorKind::ConnectionRefused
        | ErrorKind::PermissionDenied
        | ErrorKind::ConnectionReset
        | ErrorKind::BrokenPipe => TransportError::Unreachable,
        _ => TransportError::Malformed(format!("socket: {error}")),
    }
}

/// latchkey's stream as a framed tokio session over the same connection.
fn framed(stream: latchkey::Stream) -> Result<FramedSession, TransportError> {
    let std_stream = latchkey::into_fd(stream).map(StdStream::from);
    std_stream
        .and_then(|std_stream| {
            std_stream.set_nonblocking(true)?;
            UnixStream::from_std(std_stream)
        })
        .map(FramedSession::from_stream)
        .map_err(|e| unreachable_or_malformed(&e))
}

/// latchkey's errors as the carrier's: a door that cannot be reached is nobody home.
fn transport_error(error: Error) -> TransportError {
    match error {
        Error::Connect { cause, .. } | Error::Io { cause, .. } => unreachable_or_malformed(&cause),
        Error::Busy | Error::NeverAnswered(_) | Error::CannotSpawn(_) | Error::NoSelf(_) => {
            TransportError::Unreachable
        }
        other => TransportError::Malformed(format!("agent: {other}")),
    }
}

/// Knocks (latchkey's: the socket file, then a connect). Nobody home is `Unreachable`.
fn knock(door: &Door) -> Result<FramedSession, TransportError> {
    door.agent()?
        .connect()
        .map_err(transport_error)?
        .ok_or(TransportError::Unreachable)
        .and_then(framed)
}

/// Starts the agent by the app's policy, off the runtime (latchkey's wait is a blocking one), and
/// returns the connection latchkey's wait ended on. latchkey's lock decides who the agent is: a
/// second client that starts one at the same moment starts a process that finds the lock taken.
async fn start(door: &Door, how: &StartAgent) -> Result<FramedSession, TransportError> {
    let StartAgent::Spawn { args, wait } = how else {
        return Err(TransportError::Unreachable);
    };
    let agent = door.agent()?.clone();
    let (args, wait) = (args.clone(), *wait);
    let stream = tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        agent.connect_or_start(|| latchkey::spawn(&args), wait)
    })
    .await
    .map_err(|_| TransportError::Closed)?
    .map_err(transport_error)?;
    framed(stream)
}

/// A connection to the agent: knock, and start it if nobody answers and the app said to.
async fn connect(door: &Door, how: &StartAgent) -> Result<FramedSession, TransportError> {
    match knock(door) {
        Err(TransportError::Unreachable) if *how != StartAgent::Never => start(door, how).await,
        reached => reached,
    }
}

pub(super) async fn reachable(door: &Door, how: &StartAgent) -> bool {
    connect(door, how).await.is_ok()
}

/// One call: a request frame out, a reply frame back. A daemon that refuses writes the reply and
/// hangs up, so the write can fail with the reply waiting: read first, and report the write only
/// when there is nothing to read.
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

/// `OpenAuthenticated`: the request frame out, the reply frame back with the relay's descriptor
/// riding on it. A refusal brings none, and `Authenticated` exactly one.
pub(super) async fn open_authenticated(
    door: &Door,
    how: &StartAgent,
    request: AccountsRequest,
) -> Result<Relayed, TransportError> {
    let mut link = connect(door, how).await?;
    let sent = link.write_body(&request).await;
    let (reply, mut fds) = match link.read_body_with_fds::<AccountsReply>().await {
        Ok(read) => read,
        Err(read) => return Err(sent.err().unwrap_or(read)),
    };
    match (reply, fds.len()) {
        (AccountsReply::Authenticated, 1) => fds
            .pop()
            .map(|fd| Relayed::Stream(AuthenticatedStream::Fd(fd)))
            .ok_or_else(|| TransportError::Malformed("no descriptor".to_owned())),
        (AccountsReply::Refused(refusal), _) => Ok(Relayed::Refused(refusal)),
        (AccountsReply::Authenticated, n) => Err(TransportError::Malformed(format!(
            "an authenticated reply with {n} descriptors"
        ))),
        (other, _) => Err(TransportError::Malformed(format!(
            "unexpected reply to OpenAuthenticated: {other:?}"
        ))),
    }
}

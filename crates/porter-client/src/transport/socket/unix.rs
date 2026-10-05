//! The socket carrier over a Unix stream socket.

use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::env::SocketPath;
use crate::error::TransportError;
use crate::transport::framed::FramedSession;
use porter_core::{AccountsReply, AccountsRequest};
use porter_infer::LinkHello;
use std::io::ErrorKind;
use tokio::net::UnixStream;

/// The session's link: frames over the connected stream.
pub(super) type Link = FramedSession;

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

async fn connect(path: &SocketPath) -> Result<Link, TransportError> {
    UnixStream::connect(&path.0)
        .await
        .map(FramedSession::from_stream)
        .map_err(|e| unreachable_or_malformed(&e))
}

pub(super) async fn reachable(path: &SocketPath) -> bool {
    connect(path).await.is_ok()
}

/// One call: a request frame out, a reply frame back. A daemon that refuses writes the reply and
/// hangs up, so the write can fail with the reply waiting: read first, and report the write only
/// when there is nothing to read.
pub(super) async fn call(
    path: &SocketPath,
    request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    let mut link = connect(path).await?;
    let sent = link.write_body(&request).await;
    match link.read_body::<AccountsReply>().await {
        Ok(reply) => Ok(reply),
        Err(read) => Err(sent.err().unwrap_or(read).into()),
    }
}

/// A session: the hello out, then the link is the caller's.
pub(super) async fn open(path: &SocketPath, hello: LinkHello) -> Result<Link, TransportError> {
    let mut link = connect(path).await?;
    link.write_body(&hello).await?;
    Ok(link)
}

/// `OpenAuthenticated`: the request frame out, the reply frame back with the relay's descriptor
/// riding on it. A refusal brings none, and `Authenticated` exactly one.
pub(super) async fn open_authenticated(
    path: &SocketPath,
    request: AccountsRequest,
) -> Result<Relayed, TransportError> {
    let mut link = connect(path).await?;
    let sent = link.write_body(&request).await;
    let (reply, mut fds) = match link.read_body_with_fds::<AccountsReply>().await {
        Ok(read) => read,
        Err(read) => return Err(sent.err().unwrap_or(read).into()),
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

//! A daemon's start on the bus: its well-known name is the promise that its calls are answered,
//! so it claims the name only once they are.
//!
//! zbus starts a connection's object server on a task of its own the first time the server is
//! used, and that task listens for calls only once it has run. A call that reaches the connection
//! before then is dropped: no answer, no error, and the caller waits for ever. A daemon that
//! claimed its name at once lost such calls (the first caller after a start, or one that D-Bus
//! activation queued for the name). Every porter daemon registers all its objects, then waits on
//! [`serve_ready`], then calls `request_name`.

use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

/// How long one look at whether the connection takes calls may go unanswered before the next.
const LOOK: Duration = Duration::from_millis(50);

/// How long the connection may take to start taking calls.
const BOUND: Duration = Duration::from_secs(30);

/// Waits until `connection`'s object server answers method calls. The look is
/// `org.freedesktop.DBus.Peer.Ping` to the connection itself, through the bus, sent again every
/// 50 ms until one is answered; an error after 30 s with none answered.
///
/// # Errors
/// The connection has no unique name (it is not on a bus), a look failed outright, or none was
/// answered in time.
pub async fn serve_ready(connection: &Connection) -> zbus::Result<()> {
    let me = connection
        .unique_name()
        .ok_or_else(|| zbus::Error::Failure("the connection has no unique name".into()))?
        .as_str()
        .to_owned();
    let deadline = tokio::time::Instant::now() + BOUND;
    while tokio::time::Instant::now() < deadline {
        let ping = connection.call_method(
            Some(me.as_str()),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        );
        if let Ok(answer) = tokio::time::timeout(LOOK, ping).await {
            return answer.map(|_| ());
        }
    }
    Err(zbus::Error::InputOutput(Arc::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("the connection {me} took no calls"),
    ))))
}

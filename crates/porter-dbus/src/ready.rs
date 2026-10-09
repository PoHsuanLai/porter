//! A daemon's start on the bus: its well-known name is the promise that its calls are answered,
//! so it claims the name only once they are.
//!
//! zbus starts a connection's object server on a task of its own the first time the server is
//! used, and that task listens for calls only once it has run. A call that reaches the connection
//! before then is dropped: no answer, no error, and the caller waits for ever. A daemon that
//! claimed its name at once lost such calls (the first caller after a start, or one that D-Bus
//! activation queued for the name). Every porter daemon registers all its objects, then waits on
//! [`serve_ready`], then calls `request_name`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use zbus::Connection;

/// How long after a look the next one is sent; it doubles up to [`LOOK_MAX`].
const LOOK: Duration = Duration::from_millis(50);

/// The longest gap between two looks.
const LOOK_MAX: Duration = Duration::from_secs(1);

/// How long the connection may take to start taking calls. A starved computer (a big build, swap
/// full) takes tens of seconds to run a task; a daemon that gave up at 30 s would exit on exactly
/// the machine that needs it to wait.
const BOUND: Duration = Duration::from_secs(300);

/// Waits until `connection`'s object server answers method calls. The look is
/// `org.freedesktop.DBus.Peer.Ping` to the connection itself, through the bus, sent again after
/// 50 ms, then at growing gaps up to a second, until one is answered. A look is never cancelled
/// for the next: on a slow machine an answer can take longer than the gap, and a look dropped
/// just before its answer would be asked again for ever. An error after five minutes with none
/// answered.
///
/// # Errors
/// The connection has no unique name (it is not on a bus), a look failed outright, or none was
/// answered in time.
pub async fn serve_ready(connection: &Connection) -> zbus::Result<()> {
    ready_within(connection, LOOK, BOUND).await
}

async fn ready_within(
    connection: &Connection,
    look: Duration,
    bound: Duration,
) -> zbus::Result<()> {
    let me = connection
        .unique_name()
        .ok_or_else(|| zbus::Error::Failure("the connection has no unique name".into()))?
        .as_str()
        .to_owned();
    let answer = first_answer(look, bound, || {
        connection.call_method(
            Some(me.as_str()),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
    })
    .await;
    match answer {
        Some(answer) => answer.map(|_| ()),
        None => Err(zbus::Error::InputOutput(Arc::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("the connection {me} took no calls"),
        )))),
    }
}

/// Starts `ask()` now, again after `look`, again after twice that (up to [`LOOK_MAX`]) and so
/// on, keeping every one running; gives the first answer to arrive, or `None` once `bound` has
/// passed with none.
async fn first_answer<F, Fut, T>(look: Duration, bound: Duration, mut ask: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = T>,
{
    let deadline = tokio::time::Instant::now() + bound;
    let mut asked: Vec<Pin<Box<Fut>>> = Vec::new();
    let mut gap = look;
    loop {
        asked.push(Box::pin(ask()));
        let turn = (tokio::time::Instant::now() + gap).min(deadline);
        let next = tokio::time::sleep_until(turn);
        tokio::pin!(next);
        let answered = std::future::poll_fn(|cx| {
            for one in &mut asked {
                if let Poll::Ready(answer) = one.as_mut().poll(cx) {
                    return Poll::Ready(Some(answer));
                }
            }
            match next.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
        if answered.is_some() {
            return answered;
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        gap = (gap * 2).min(LOOK_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn an_answer_slower_than_the_gap_between_looks_is_still_taken() {
        // Every look is answered after 400 ms: with each look cancelled at the next (50 ms), no
        // answer would ever arrive.
        let answer = first_answer(
            Duration::from_millis(50),
            Duration::from_secs(30),
            || async {
                tokio::time::sleep(Duration::from_millis(400)).await;
                7
            },
        )
        .await;
        assert_eq!(answer, Some(7));
    }

    #[tokio::test(start_paused = true)]
    async fn a_start_that_takes_a_minute_is_waited_for_and_one_that_never_comes_ends() {
        let began = tokio::time::Instant::now();
        let late = began + Duration::from_secs(60);
        // A look is answered only once the server is up (a minute in), as zbus drops the ones
        // that come before.
        let answer = first_answer(LOOK, BOUND, || async move {
            tokio::time::sleep_until(late).await;
            "up"
        })
        .await;
        assert_eq!(answer, Some("up"));
        assert!(began.elapsed() < Duration::from_secs(62));

        let never = first_answer(LOOK, Duration::from_secs(10), || {
            std::future::pending::<()>()
        })
        .await;
        assert_eq!(never, None);
    }
}

//! A session's failure boundary: the session runs as its own task, and if that task panics the
//! person's end of that one session is told so in plain words and the daemon goes on. Other
//! sessions are tasks of their own and never see it.

use super::wire::Wire;
use porter_infer::{InferEvent, InferReply, ModelError};
use std::future::Future;
use tokio::net::UnixStream;

/// Runs `session` as a task of its own. If it panics, the panic ends here: `log` is called once
/// (it says what the machine looked like, for the log), the client on `report` (another handle on
/// the session's socket) is sent the end of its turn as `Interrupted`, and this returns. A
/// session that ends any other way (it finished, or was aborted) is left alone.
pub async fn guarded<F>(session: F, report: UnixStream, log: impl FnOnce() -> String)
where
    F: Future<Output = ()> + Send + 'static,
{
    let Err(ended) = tokio::spawn(session).await else {
        return;
    };
    if !ended.is_panic() {
        return;
    }
    eprintln!("inferd: a session stopped unexpectedly; {}", log());
    let end = InferEvent::Finished(InferReply::Failed(ModelError::Interrupted));
    // Best effort: the client may be gone already.
    let _ = Wire::new(report).write(&end).await;
}

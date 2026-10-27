//! Ordered async bytes, without a runtime: the seam an authenticated relay reads and writes
//! (`porter-proxy`), and an in-memory pair of ends for where there is no socketpair (an app
//! hosting porter in process, Windows). Nothing here opens a socket or spawns a task; a host
//! that has a runtime implements [`ByteStream`] over its own streams.

use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

/// One end of an ordered, reliable byte stream.
pub trait ByteStream: Send {
    /// Reads at least one byte into `buf`, or `0` once the other end has finished writing.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;

    /// Writes all of `bytes`.
    fn write_all(&mut self, bytes: &[u8]) -> impl Future<Output = io::Result<()>> + Send;

    /// Tells the other end nothing more will be written.
    fn shutdown(&mut self) -> impl Future<Output = io::Result<()>> + Send;
}

/// One direction of a duplex: what was written and not yet read.
#[derive(Debug, Default)]
struct Lane {
    bytes: VecDeque<u8>,
    finished: bool,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

type Shared = Arc<Mutex<Lane>>;

fn locked(lane: &Shared) -> MutexGuard<'_, Lane> {
    // Every critical section is a plain queue update that cannot panic midway.
    lane.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One end of an in-memory duplex. Dropping an end finishes what it writes.
#[derive(Debug)]
pub struct DuplexEnd {
    incoming: Shared,
    outgoing: Shared,
    capacity: usize,
}

/// Two connected ends; each holds at most `capacity` unread bytes per direction (at least one),
/// and a write past that waits for the reader.
pub fn duplex(capacity: usize) -> (DuplexEnd, DuplexEnd) {
    let (a_to_b, b_to_a) = (Shared::default(), Shared::default());
    let capacity = capacity.max(1);
    let a = DuplexEnd {
        incoming: Arc::clone(&b_to_a),
        outgoing: Arc::clone(&a_to_b),
        capacity,
    };
    let b = DuplexEnd {
        incoming: a_to_b,
        outgoing: b_to_a,
        capacity,
    };
    (a, b)
}

impl Drop for DuplexEnd {
    fn drop(&mut self) {
        let mut lane = locked(&self.outgoing);
        lane.finished = true;
        if let Some(reader) = lane.reader.take() {
            reader.wake();
        }
        drop(lane);
        // The other end's writes now go nowhere: wake it so it sees the broken pipe.
        if let Some(writer) = locked(&self.incoming).writer.take() {
            writer.wake();
        }
    }
}

impl DuplexEnd {
    /// Reads what is waiting into `buf`: the number of bytes, or `0` once the other end has
    /// finished writing. Pending (the waker is kept) while nothing is waiting. The poll form
    /// of [`ByteStream::read`], for a host that drives reading and writing from one task.
    pub fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut lane = locked(&self.incoming);
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let n = buf.len().min(lane.bytes.len());
        if n > 0 {
            for (slot, byte) in buf.iter_mut().zip(lane.bytes.drain(..n)) {
                *slot = byte;
            }
            if let Some(writer) = lane.writer.take() {
                writer.wake();
            }
            return Poll::Ready(Ok(n));
        }
        if lane.finished {
            return Poll::Ready(Ok(0));
        }
        lane.reader = Some(cx.waker().clone());
        Poll::Pending
    }

    /// Writes as much of `bytes` as fits, and returns how many: at least one unless `bytes` is
    /// empty. Pending (the waker is kept) while the other end has not read enough to make room;
    /// a broken pipe once the other end is dropped. The poll form of [`ByteStream::write_all`].
    pub fn poll_write(&mut self, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut lane = locked(&self.outgoing);
        if Arc::strong_count(&self.outgoing) < 2 {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let room = self.capacity.saturating_sub(lane.bytes.len());
        if room == 0 {
            lane.writer = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let now = room.min(bytes.len());
        lane.bytes.extend(&bytes[..now]);
        if let Some(reader) = lane.reader.take() {
            reader.wake();
        }
        Poll::Ready(Ok(now))
    }

    /// Tells the other end nothing more will be written.
    pub fn shutdown_now(&mut self) {
        let mut lane = locked(&self.outgoing);
        lane.finished = true;
        if let Some(reader) = lane.reader.take() {
            reader.wake();
        }
    }
}

impl ByteStream for DuplexEnd {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.poll_read(cx, buf)).await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let n = poll_fn(|cx| self.poll_write(cx, rest)).await?;
            rest = &rest[n..];
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.shutdown_now();
        Ok(())
    }
}

#[cfg(test)]
mod tests;

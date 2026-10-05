use super::*;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Wake};

struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Polls `future` once with a waker that counts its wakes.
fn poll_once<F: Future>(future: &mut std::pin::Pin<&mut F>, wakes: &Arc<Count>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::clone(wakes));
    future.as_mut().poll(&mut Context::from_waker(&waker))
}

fn ready<F: Future>(future: F) -> F::Output {
    let wakes = Arc::new(Count(AtomicUsize::new(0)));
    let mut future = pin!(future);
    match poll_once(&mut future, &wakes) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the future was not ready"),
    }
}

#[test]
fn what_one_end_writes_the_other_reads_in_order() {
    let (mut app, mut relay) = duplex(64);
    ready(app.write_all(b"A001 LOGIN")).expect("write");
    ready(app.write_all(b" x y\r\n")).expect("write");
    let mut buf = [0u8; 64];
    let n = ready(relay.read(&mut buf)).expect("read");
    assert_eq!(&buf[..n], b"A001 LOGIN x y\r\n");
    ready(relay.write_all(b"* OK\r\n")).expect("write");
    let n = ready(app.read(&mut buf)).expect("read");
    assert_eq!(&buf[..n], b"* OK\r\n");
}

#[test]
fn a_read_waits_until_something_is_written_and_is_woken() {
    let (mut app, mut relay) = duplex(8);
    let wakes = Arc::new(Count(AtomicUsize::new(0)));
    let mut buf = [0u8; 8];
    let mut read = pin!(relay.read(&mut buf));
    assert!(poll_once(&mut read, &wakes).is_pending());
    assert_eq!(wakes.0.load(Ordering::SeqCst), 0);
    ready(app.write_all(b"x")).expect("write");
    assert_eq!(
        wakes.0.load(Ordering::SeqCst),
        1,
        "the write woke the reader"
    );
    assert!(matches!(poll_once(&mut read, &wakes), Poll::Ready(Ok(1))));
}

#[test]
fn a_full_lane_holds_the_writer_until_the_reader_drains_it() {
    let (mut app, mut relay) = duplex(4);
    let wakes = Arc::new(Count(AtomicUsize::new(0)));
    let mut write = pin!(app.write_all(b"abcdefgh"));
    assert!(poll_once(&mut write, &wakes).is_pending());
    let mut buf = [0u8; 8];
    assert_eq!(ready(relay.read(&mut buf)).expect("read"), 4);
    assert_eq!(
        wakes.0.load(Ordering::SeqCst),
        1,
        "the read woke the writer"
    );
    assert!(matches!(poll_once(&mut write, &wakes), Poll::Ready(Ok(()))));
    assert_eq!(ready(relay.read(&mut buf)).expect("read"), 4);
    assert_eq!(&buf[..4], b"efgh");
}

#[test]
fn shutdown_and_dropping_end_the_stream_after_what_was_written() {
    let (mut app, mut relay) = duplex(16);
    ready(app.write_all(b"bye")).expect("write");
    ready(app.shutdown()).expect("shutdown");
    let mut buf = [0u8; 8];
    assert_eq!(ready(relay.read(&mut buf)).expect("read"), 3);
    assert_eq!(ready(relay.read(&mut buf)).expect("read"), 0);
    drop(app);
    let err = ready(relay.write_all(b"x")).expect_err("the app is gone");
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

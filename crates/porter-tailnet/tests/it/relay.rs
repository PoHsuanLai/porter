//! A local path that leads to one of the persons's computers, checked on every connection.

use super::common::{addresses, fake, free_port, network, scratch};
use porter_core::NodeId;
use porter_fake::GENEROUS;
use porter_fake_servers::Daemon;
use porter_tailnet::{DialError, Dialer, OnFault, Relay};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixStream};

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a node id")
}

/// A computer that answers whatever it reads with "pong" and counts connections.
async fn pong_at(at: SocketAddr) -> Arc<Mutex<usize>> {
    let listener = TcpListener::bind(at).await.unwrap();
    let count = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&count);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            *counted.lock().unwrap() += 1;
            tokio::spawn(async move {
                let mut buffer = [0_u8; 16];
                while let Ok(n) = stream.read(&mut buffer).await {
                    if n == 0 || stream.write_all(b"pong").await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    count
}

async fn through(path: &std::path::Path) -> String {
    let mut stream = UnixStream::connect(path).await.expect("the relay's path");
    stream.write_all(b"ping").await.unwrap();
    let mut answer = vec![0_u8; 4096];
    let n = tokio::time::timeout(GENEROUS, stream.read(&mut answer))
        .await
        .expect("an answer within the generous wait")
        .unwrap();
    String::from_utf8_lossy(&answer[..n]).into_owned()
}

#[tokio::test]
async fn bytes_go_both_ways_and_each_connection_asks_tailscale_again() {
    let a = addresses();
    let port = free_port();
    let (fake, api) = fake("relay", Daemon::Running(network(&a))).await;
    let count = pong_at(SocketAddr::new(a.pi, port)).await;
    let path = scratch("relay-path").join("pi.sock");
    let faults = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::clone(&faults);
    let on_fault: OnFault = Arc::new(move |fault: &DialError| told.lock().unwrap().push(*fault));
    let _relay = Relay::start(&path, node("nPI"), Dialer::new(api, port), on_fault).expect("relay");

    assert_eq!(through(&path).await, "pong");
    assert_eq!(*count.lock().unwrap(), 1);

    // The computer is tagged now: the next connection is turned away, with the reason in words,
    // and nothing reaches it.
    fake.edit(|net| net.peers[0].tags.push("tag:ci".into()));
    let refused = through(&path).await;
    assert!(refused.starts_with("HTTP/1.1 503"), "{refused}");
    assert!(
        refused.contains("That computer is not one of your own."),
        "{refused}"
    );
    assert_eq!(*count.lock().unwrap(), 1);
    assert_eq!(faults.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_path_left_by_an_earlier_run_is_replaced() {
    let a = addresses();
    let port = free_port();
    let (_fake, api) = fake("relay-stale", Daemon::Running(network(&a))).await;
    let count = pong_at(SocketAddr::new(a.pi, port)).await;
    let path = scratch("relay-stale-path").join("pi.sock");
    std::fs::write(&path, "left over").unwrap();
    let nothing: OnFault = Arc::new(|_| {});
    let _relay = Relay::start(&path, node("nPI"), Dialer::new(api, port), nothing).expect("relay");
    assert_eq!(through(&path).await, "pong");
    assert_eq!(*count.lock().unwrap(), 1);
}

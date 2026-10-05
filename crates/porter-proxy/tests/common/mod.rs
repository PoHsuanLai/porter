//! What the relay tests share: the app's end of a relay, the plans, and a minimal fake
//! ManageSieve server (porter-fake-servers has none).

#![allow(dead_code)]

pub mod sieve;

use porter_core::{
    CapabilityKind, EndpointUrl, Family, LoginName, RelayAuth, RelayPlan, SecretText,
    ServiceEndpoint, Tls,
};
use porter_fake::FakeAddress;
use porter_fake_servers::tls;
use porter_proxy::{RelayEnd, RustlsConnect, TokioStream, relay};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

pub const USER: &str = "alice@fake.test";
pub const PASSWORD: &str = "hunter2";
pub const TOKEN: &str = "fake-access-9";
const WAIT: Duration = Duration::from_secs(10);

pub fn port(address: &FakeAddress) -> u16 {
    match address {
        FakeAddress::Loopback(port) => *port,
        FakeAddress::Socket(_) => panic!("a loopback fake"),
    }
}

pub fn plan(family: Family, url: &str, tls: Tls, auth: RelayAuth) -> RelayPlan {
    RelayPlan {
        endpoint: ServiceEndpoint {
            family,
            url: EndpointUrl::parse(url).expect("url"),
            tls,
            login: LoginName(USER.into()),
        },
        kind: CapabilityKind::Mail,
        auth,
    }
}

pub fn password() -> RelayAuth {
    RelayAuth::Password(SecretText::new(PASSWORD))
}

pub fn token() -> RelayAuth {
    RelayAuth::AccessToken(SecretText::new(TOKEN))
}

/// A connector trusting the fakes' scratch CA and nothing else.
pub fn trusting_fakes() -> RustlsConnect {
    RustlsConnect::trusting([tls::ca_der()])
}

/// The app's end of a running relay, and everything it has received so far.
pub struct App {
    stream: DuplexStream,
    pub received: Vec<u8>,
    cursor: usize,
    task: JoinHandle<RelayEnd>,
}

pub fn start(plan: RelayPlan, connect: RustlsConnect) -> App {
    let (app, relay_end) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move { relay(plan, TokioStream(relay_end), &connect).await });
    App {
        stream: app,
        received: Vec::new(),
        cursor: 0,
        task,
    }
}

impl App {
    pub async fn send(&mut self, text: &str) {
        self.send_bytes(text.as_bytes()).await;
    }

    pub async fn send_bytes(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("the relay is up");
    }

    /// Reads until `marker` has arrived; returns what came since the last call, through the
    /// marker.
    pub async fn read_until(&mut self, marker: &str) -> String {
        loop {
            let fresh = &self.received[self.cursor..];
            if let Some(at) = find(fresh, marker.as_bytes()) {
                let end = self.cursor + at + marker.len();
                let text = String::from_utf8_lossy(&self.received[self.cursor..end]).into_owned();
                self.cursor = end;
                return text;
            }
            let mut buf = [0u8; 4096];
            let n = timeout(WAIT, self.stream.read(&mut buf))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "waiting for {marker:?}; got {:?}",
                        String::from_utf8_lossy(fresh)
                    )
                })
                .expect("read");
            assert!(
                n > 0,
                "the relay closed while waiting for {marker:?}; got {:?}",
                String::from_utf8_lossy(&self.received[self.cursor..])
            );
            self.received.extend_from_slice(&buf[..n]);
        }
    }

    /// Reads until `n` more bytes have arrived; returns them.
    pub async fn read_bytes(&mut self, n: usize) -> String {
        while self.received.len() - self.cursor < n {
            let mut buf = [0u8; 4096];
            let got = timeout(WAIT, self.stream.read(&mut buf))
                .await
                .expect("bytes arrive")
                .expect("read");
            assert!(got > 0, "the relay closed with {n} bytes awaited");
            self.received.extend_from_slice(&buf[..got]);
        }
        let text =
            String::from_utf8_lossy(&self.received[self.cursor..self.cursor + n]).into_owned();
        self.cursor += n;
        text
    }

    /// Reads to the end of the stream; what arrived since the last call.
    pub async fn read_to_end(&mut self) -> String {
        let mut rest = Vec::new();
        timeout(WAIT, self.stream.read_to_end(&mut rest))
            .await
            .expect("the relay ends")
            .expect("read");
        self.received.extend_from_slice(&rest);
        let text = String::from_utf8_lossy(&self.received[self.cursor..]).into_owned();
        self.cursor = self.received.len();
        text
    }

    /// Ends the app's side and returns how the relay ended.
    pub async fn finish(mut self) -> RelayEnd {
        let _ = self.stream.shutdown().await;
        self.ended().await
    }

    /// How the relay ended, without closing the app's side.
    pub async fn ended(self) -> RelayEnd {
        timeout(WAIT, self.task)
            .await
            .expect("the relay ends")
            .expect("the relay task")
    }

    /// Every byte the app has received, as text.
    pub fn everything(&self) -> String {
        String::from_utf8_lossy(&self.received).into_owned()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

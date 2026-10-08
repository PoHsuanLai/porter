//! The mail servers a generic sign-in tries its password at: a fake IMAP and a fake POP3 server
//! per connection security, one planted login, and a connector that sends whatever the account
//! names (`imap.fake.test:993`, `mail.fake.test:1143`) to the fake of the same protocol and
//! security on this computer.
#![allow(dead_code)]

use porter_core::{EndpointProtocol, Origin, Tls};
use porter_fake_servers::net::{Bind, port_of};
use porter_fake_servers::{Accounts, FakeImap, FakePop3, MailHandle, Running, mailbox, tls};
use porter_proxy::{Connect, ConnectFault, NetStream, RustlsConnect};

/// The fakes, kept running for as long as this is held.
pub struct MailWorld {
    wire: Wire,
    _servers: Vec<Running<MailHandle>>,
}

/// The ports of the fakes by connection security: implicit TLS, STARTTLS, plain.
#[derive(Debug, Clone, Copy)]
struct Ports {
    implicit: u16,
    starttls: u16,
    plain: u16,
}

impl Ports {
    fn of(&self, tls: Tls) -> u16 {
        match tls {
            Tls::Implicit => self.implicit,
            Tls::StartTls => self.starttls,
            Tls::Plain => self.plain,
        }
    }
}

/// A connector that reaches the fakes and trusts their scratch CA and nothing else.
#[derive(Debug, Clone)]
pub struct Wire {
    inner: RustlsConnect,
    imap: Ports,
    pop3: Ports,
}

impl MailWorld {
    /// An IMAP and a POP3 fake per security, each letting `user` in with `password`.
    pub async fn start(user: &str, password: &str) -> Self {
        let accounts = Accounts::password(user, password);
        let mut servers = Vec::new();
        let mut ports = Vec::new();
        for imap in [true, false] {
            for tls in [Tls::Implicit, Tls::StartTls, Tls::Plain] {
                let running = match imap {
                    true => {
                        FakeImap::start(&Bind::Loopback, tls, accounts.clone(), mailbox(1)).await
                    }
                    false => {
                        FakePop3::start(&Bind::Loopback, tls, accounts.clone(), mailbox(1)).await
                    }
                }
                .expect("fake mail server");
                ports.push(port_of(running.address()));
                servers.push(running);
            }
        }
        let of = |at: usize| Ports {
            implicit: ports[at],
            starttls: ports[at + 1],
            plain: ports[at + 2],
        };
        Self {
            wire: Wire {
                inner: RustlsConnect::trusting([tls::ca_der()]),
                imap: of(0),
                pop3: of(3),
            },
            _servers: servers,
        }
    }

    /// The connector to give the provider.
    pub fn wire(&self) -> Wire {
        self.wire.clone()
    }
}

impl Wire {
    /// A connector whose every port is closed: the servers cannot be reached.
    pub fn nowhere() -> Self {
        let closed = || {
            let held = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            held.local_addr().expect("addr").port()
        };
        let ports = Ports {
            implicit: closed(),
            starttls: closed(),
            plain: closed(),
        };
        Self {
            inner: RustlsConnect::trusting([tls::ca_der()]),
            imap: ports,
            pop3: ports,
        }
    }

    fn here(&self, origin: &Origin, tls: Tls) -> Origin {
        let ports = match origin.scheme.protocol() {
            EndpointProtocol::Pop3 => self.pop3,
            _ => self.imap,
        };
        Origin {
            scheme: origin.scheme,
            host: "127.0.0.1".to_owned(),
            port: ports.of(tls),
        }
    }
}

impl Connect for Wire {
    type Stream = NetStream;

    async fn dial(&self, origin: &Origin, tls: Tls) -> Result<NetStream, ConnectFault> {
        self.inner.dial(&self.here(origin, tls), tls).await
    }

    async fn upgrade(&self, stream: NetStream, _host: &str) -> Result<NetStream, ConnectFault> {
        self.inner.upgrade(stream, "127.0.0.1").await
    }
}

//! A minimal fake ManageSieve server (RFC 5804): STARTTLS, `AUTHENTICATE "PLAIN"`, and enough
//! of LISTSCRIPTS and LOGOUT to see a session work. It records every login it was shown.

use super::{PASSWORD, USER};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_fake::FakeAddress;
use porter_fake_servers::mail::Lines;
use porter_fake_servers::net::{Bind, Conn, Listener};
use porter_fake_servers::tls;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

/// One login attempt the fake saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub user: String,
    pub password: String,
    pub accepted: bool,
    pub secure: bool,
}

pub struct FakeSieve {
    pub address: FakeAddress,
    pub logins: Arc<Mutex<Vec<Login>>>,
    task: JoinHandle<()>,
}

impl Drop for FakeSieve {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeSieve {
    pub async fn start() -> Self {
        let listener = Listener::bind(&Bind::Loopback, "sieve")
            .await
            .expect("bind");
        let address = listener.address().clone();
        let logins = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&logins);
        let task = tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let _ = session(conn, seen).await;
                });
            }
        });
        Self {
            address,
            logins,
            task,
        }
    }

    pub fn logins(&self) -> Vec<Login> {
        self.logins.lock().unwrap().clone()
    }
}

const PLAIN_CAPS: &str = "\"IMPLEMENTATION\" \"fake sieve\"\r\n\"SIEVE\" \"fileinto vacation\"\r\n\"STARTTLS\"\r\n\"VERSION\" \"1.0\"\r\nOK \"fake ready\"\r\n";
const SECURE_CAPS: &str = "\"IMPLEMENTATION\" \"fake sieve\"\r\n\"SIEVE\" \"fileinto vacation\"\r\n\"SASL\" \"PLAIN\"\r\n\"VERSION\" \"1.0\"\r\nOK \"fake ready\"\r\n";

async fn session(conn: Conn, seen: Arc<Mutex<Vec<Login>>>) -> std::io::Result<()> {
    let mut lines = Lines::new(conn);
    lines.send_raw(PLAIN_CAPS).await?;
    // Before TLS: only STARTTLS (and LOGOUT) are answered.
    while let Some(line) = lines.read_line().await? {
        match line.to_ascii_uppercase().as_str() {
            "STARTTLS" => {
                lines.send("OK \"Begin TLS negotiation now.\"").await?;
                let conn = lines.into_stream().expect("no bytes past STARTTLS");
                let stream = tls::acceptor().accept(conn).await?;
                return secure(Lines::new(stream), seen).await;
            }
            "LOGOUT" => {
                lines.send("OK \"bye\"").await?;
                return Ok(());
            }
            _ => lines.send("NO \"STARTTLS first\"").await?,
        }
    }
    Ok(())
}

async fn secure<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut lines: Lines<S>,
    seen: Arc<Mutex<Vec<Login>>>,
) -> std::io::Result<()> {
    lines.send_raw(SECURE_CAPS).await?;
    let mut authed = false;
    while let Some(line) = lines.read_line().await? {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("AUTHENTICATE \"PLAIN\"") && !authed {
            let initial = line.split('"').nth(3).unwrap_or_default().to_owned();
            let decoded = STANDARD
                .decode(initial)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            let mut parts = decoded.split('\0').skip(1);
            let (user, password) = (
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
            );
            let accepted = user == USER && password == PASSWORD;
            seen.lock().unwrap().push(Login {
                user,
                password,
                accepted,
                secure: true,
            });
            authed = accepted;
            match accepted {
                true => lines.send("OK \"Authenticated\"").await?,
                false => lines.send("NO \"Authentication failed\"").await?,
            }
        } else if upper == "LISTSCRIPTS" && authed {
            lines
                .send_raw("\"main\" ACTIVE\r\n\"vacation\"\r\nOK \"Listscripts completed.\"\r\n")
                .await?;
        } else if upper == "LOGOUT" {
            lines.send("OK \"bye\"").await?;
            return Ok(());
        } else {
            lines.send("NO \"not now\"").await?;
        }
    }
    Ok(())
}

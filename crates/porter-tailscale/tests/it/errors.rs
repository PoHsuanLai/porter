use crate::common::{LOGIN_PAGE, network, scratch, start};
use porter_fake_servers::Daemon;
use porter_tailscale::{LocalApi, TailscaleError};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

#[tokio::test]
async fn no_socket_and_no_program_is_not_installed() {
    let dir = scratch("errors-not-installed");
    let api = LocalApi::new(dir.join("tailscaled.sock")).with_program_dirs(vec![dir.clone()]);
    assert_eq!(api.status().await, Err(TailscaleError::NotInstalled));
    assert_eq!(
        TailscaleError::NotInstalled.to_string(),
        "Tailscale isn't installed on this computer."
    );
}

#[tokio::test]
async fn no_socket_but_the_program_is_there_is_not_running() {
    let dir = scratch("errors-installed-not-running");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(bin.join("tailscaled"), b"#!/bin/sh\n").expect("program");
    let api = LocalApi::new(dir.join("tailscaled.sock")).with_program_dirs(vec![bin]);
    assert_eq!(api.status().await, Err(TailscaleError::NotRunning));
}

#[tokio::test]
async fn a_socket_nobody_listens_on_is_not_running() {
    let (fake, api) = start("errors-stopped", Daemon::Running(network())).await;
    assert!(api.status().await.is_ok());
    fake.stop();
    // Let the aborted server go before dialling its leftover socket.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(api.status().await, Err(TailscaleError::NotRunning));
    assert_eq!(api.start_login().await, Err(TailscaleError::NotRunning));
    // And it comes back where it was.
    fake.restart().await.expect("restart");
    assert!(api.status().await.is_ok());
}

#[tokio::test]
async fn a_daemon_that_answers_forbidden_is_refused_for_every_question() {
    let (_fake, api) = start("errors-refusing", Daemon::Refusing).await;
    assert_eq!(api.status().await, Err(TailscaleError::Refused));
    let who = "100.64.0.2:4000".parse().expect("addr");
    assert_eq!(api.whois(who).await, Err(TailscaleError::Refused));
    assert_eq!(api.start_login().await, Err(TailscaleError::Refused));
    assert_eq!(api.watch().await.err(), Some(TailscaleError::Refused));
    assert_eq!(
        TailscaleError::Refused.to_string(),
        "This computer's Tailscale doesn't let your accounts ask it yet."
    );
}

#[tokio::test]
async fn a_user_who_may_read_but_not_sign_in_is_refused_the_sign_in_only() {
    let (fake, api) = start(
        "errors-read-only",
        Daemon::SignedOut {
            auth_url: LOGIN_PAGE.to_owned(),
        },
    )
    .await;
    fake.set_read_only(true);
    assert!(api.status().await.is_ok());
    assert_eq!(api.start_login().await, Err(TailscaleError::Refused));
    assert_eq!(fake.logins(), 0);
}

#[tokio::test]
async fn a_socket_that_is_not_tailscale_is_malformed() {
    let dir = scratch("errors-malformed");
    let path = dir.join("tailscaled.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                // A well-formed HTTP answer whose body is not a status.
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
                    )
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    let api = LocalApi::new(&path).with_program_dirs(Vec::new());
    assert_eq!(api.status().await, Err(TailscaleError::Malformed));
}

#[tokio::test]
async fn a_daemon_that_never_answers_times_out() {
    let dir = scratch("errors-timeout");
    let path = dir.join("tailscaled.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    // Accepts and says nothing.
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            open.push(stream);
        }
    });
    let api = LocalApi::new(&path)
        .with_program_dirs(Vec::new())
        .with_timeout(std::time::Duration::from_millis(200));
    assert_eq!(api.status().await, Err(TailscaleError::TimedOut));
    held.abort();
}

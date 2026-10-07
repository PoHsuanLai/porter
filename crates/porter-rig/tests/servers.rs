//! `porter-rig-servers` as a process: it starts, writes `rig.json`, answers its control
//! endpoint and the fakes it started, plants the secrets it says, and ends cleanly on SIGTERM
//! sent to its PID.

mod common;

use common::{Proc, eventually, scratch};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, post_form, send};
use porter_rig::control::call;
use porter_rig::planted;
use porter_rig::rigfile::RigFile;
use serde_json::Value;
use std::net::TcpStream;

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON")
}

async fn graph_status(url: &str, bearer: &str) -> u16 {
    let (address, _) = split_loopback(url).expect("address");
    let request = Request::new("GET", "/v1.0/me/drive")
        .with_header("Authorization", &format!("Bearer {bearer}"));
    send(&address, Scheme::Http, &request)
        .await
        .expect("request")
        .status
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rig_starts_lists_what_it_planted_serves_its_levers_and_ends_on_sigterm() {
    let dir = scratch("servers");
    let mut rig = Proc::spawn(
        env!("CARGO_BIN_EXE_porter-rig-servers"),
        &[
            "--dir",
            dir.to_str().expect("utf-8"),
            "--graph",
            "--imap",
            "--smtp",
            "--pop3",
            "--dav",
            "--nextcloud",
            "--ollama",
            "--llm-api",
        ],
        &[],
    );
    let file = eventually("rig.json is written", || RigFile::read(&dir).ok()).await;

    // What it says: its own PID, the CA, ports of an ephemeral kind, every planted secret.
    assert_eq!(file.pid, rig.pid());
    assert!(file.ca.is_file());
    assert_eq!(file.secrets.len(), planted::ALL.len());
    for secret in planted::ALL {
        assert!(file.secrets.iter().any(|s| s == secret), "{secret}");
    }
    let oauth = file.oauth.clone().expect("--graph starts the issuer");
    let graph = file.graph.clone().expect("graph");
    let ollama = file.ollama.clone().expect("ollama");
    let mut ports = vec![oauth.port, graph.port, ollama.port];
    for mail in [&file.imap, &file.smtp, &file.pop3] {
        let mail = mail.as_ref().expect("mail fake");
        assert_eq!(
            (mail.host.as_str(), mail.tls.as_str()),
            ("127.0.0.1", "implicit")
        );
        assert_eq!(mail.password, planted::MAIL_PASSWORD);
        ports.push(mail.port);
    }
    ports.extend([
        file.dav.as_ref().expect("dav").port,
        file.nextcloud.as_ref().expect("nextcloud").port,
        file.llm_api.as_ref().expect("llm api").port,
    ]);
    assert!(ports.iter().all(|p| *p >= 1024), "{ports:?}");
    for port in &ports {
        TcpStream::connect(("127.0.0.1", *port)).expect("every fake listens");
    }
    assert_eq!(oauth.access_token, planted::OAUTH_ACCESS_TOKEN);
    assert_eq!(oauth.refresh_token, planted::OAUTH_REFRESH_TOKEN);
    assert_eq!(
        file.llm_api.as_ref().expect("llm").api_key,
        planted::API_KEY
    );

    // One fake: the Graph drive takes the planted access token (it asks the issuer) and no other.
    assert_eq!(
        graph_status(&graph.url, planted::OAUTH_ACCESS_TOKEN).await,
        200
    );
    assert_eq!(graph_status(&graph.url, "not-a-token").await, 401);

    // Levers: a remote edit shows on the drive, a stopped Ollama refuses and comes back at
    // its port, a refused refresh and a revoked refresh token are what the issuer answers.
    let put = call(
        &file.control,
        "POST",
        "/graph/put?path=notes/a.txt",
        b"remote",
    )
    .await;
    assert_eq!(put.expect("put").status, 200);
    let got = call(&file.control, "GET", "/graph/file?path=notes/a.txt", b"")
        .await
        .expect("get");
    assert_eq!(got.body, b"remote");
    let files = call(&file.control, "GET", "/graph/files", b"")
        .await
        .expect("files");
    assert_eq!(json(&files.text())["files"][0]["path"], "notes/a.txt");
    let gone = call(&file.control, "POST", "/graph/delete?path=notes/a.txt", b"")
        .await
        .expect("delete");
    assert_eq!(gone.status, 200);
    let missing = call(&file.control, "GET", "/graph/file?path=notes/a.txt", b"")
        .await
        .expect("get");
    assert_eq!(missing.status, 404);

    let stopped = call(&file.control, "POST", "/ollama/stop", b"")
        .await
        .expect("stop");
    assert_eq!(json(&stopped.text())["stopped"], true);
    assert!(
        TcpStream::connect(("127.0.0.1", ollama.port)).is_err(),
        "stopped"
    );
    let status = call(&file.control, "GET", "/ollama/status", b"")
        .await
        .expect("status");
    assert_eq!(json(&status.text())["running"], false);
    let started = call(&file.control, "POST", "/ollama/start", b"")
        .await
        .expect("start");
    assert_eq!(json(&started.text())["port"], ollama.port);
    TcpStream::connect(("127.0.0.1", ollama.port)).expect("back at the same port");

    let (issuer, _) = split_loopback(&oauth.url).expect("address");
    let refresh = |token: &str| {
        let pairs = [
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", planted::OAUTH_CLIENT),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let issuer = issuer.clone();
        async move {
            let pairs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            post_form(&issuer, "/token", &pairs)
                .await
                .expect("token")
                .status
        }
    };
    let refused = call(&file.control, "POST", "/issuer/refuse-refreshes?n=1", b"")
        .await
        .expect("lever");
    assert_eq!(refused.status, 200);
    assert_eq!(
        refresh(planted::OAUTH_REFRESH_TOKEN).await,
        400,
        "refused once"
    );
    let revoked = call(&file.control, "POST", "/issuer/revoke", b"")
        .await
        .expect("revoke");
    assert_eq!(revoked.status, 200);
    assert_eq!(
        refresh(planted::OAUTH_REFRESH_TOKEN).await,
        400,
        "a revoked refresh token is not honoured"
    );
    assert_eq!(
        call(&file.control, "POST", "/nope", b"")
            .await
            .expect("404")
            .status,
        404
    );
    let again = call(&file.control, "GET", "/rig", b"").await.expect("rig");
    assert_eq!(json(&again.text())["pid"], rig.pid());

    // SIGTERM to its PID ends it cleanly and takes rig.json with it.
    let status = rig.terminate();
    assert!(status.success(), "{status:?}");
    assert!(!RigFile::path(&dir).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rig_asked_for_nothing_starts_only_its_control_endpoint_and_a_lever_on_a_missing_fake_is_a_conflict()
 {
    let dir = scratch("servers-bare");
    let mut rig = Proc::spawn(
        env!("CARGO_BIN_EXE_porter-rig-servers"),
        &["--dir", dir.to_str().expect("utf-8")],
        &[],
    );
    let file = eventually("rig.json is written", || RigFile::read(&dir).ok()).await;
    assert!(file.imap.is_none() && file.graph.is_none() && file.oauth.is_none());
    let lever = call(&file.control, "POST", "/ollama/stop", b"")
        .await
        .expect("lever");
    assert_eq!(lever.status, 409);
    let stop = call(&file.control, "POST", "/stop", b"")
        .await
        .expect("stop");
    assert_eq!(stop.status, 200);
    let status = tokio::task::spawn_blocking(move || rig.child.wait().expect("ends"))
        .await
        .expect("join");
    assert!(status.success(), "POST /stop ends it cleanly");
    let _ = std::fs::remove_dir_all(dir);
}

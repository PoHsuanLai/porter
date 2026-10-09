use super::*;
use crate::attached::config::{Place, Reach};
use crate::attached::key::{KeyFile, KeyFileProblem};
use crate::lab::Lab;
use crate::testkit::Scratch;
use model_http::Port;
use porter_fake_servers::net::Bind;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SERVED: &str = "qwen3-32b";

fn socket_target(path: PathBuf, key: Option<KeyFile>) -> Target {
    Target {
        reach: Reach::Socket(path),
        key,
        place: Place::MyNetwork,
        computer: None,
    }
}

fn port_target(port: u16, key: Option<KeyFile>) -> Target {
    Target {
        reach: Reach::Loopback {
            host: std::net::Ipv4Addr::LOCALHOST,
            port: Port(port),
        },
        key,
        place: Place::ThisDevice,
        computer: None,
    }
}

fn key_file(scratch: &Scratch, text: &str, mode: u32) -> KeyFile {
    let path = scratch.path().join("lab.key");
    std::fs::write(&path, text).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    KeyFile::at(path)
}

#[tokio::test]
async fn an_engine_that_lists_the_served_name_is_ready_over_a_socket_and_over_loopback() {
    let scratch = Scratch::new("ck");
    let on_socket = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &[SERVED],
        None,
    )
    .await;
    assert_eq!(
        probe(&socket_target(on_socket.socket(), None), SERVED).await,
        Ok(())
    );
    let on_port = Lab::start(&Bind::Loopback, "lab", &["other", SERVED], None).await;
    assert_eq!(
        probe(&port_target(on_port.port(), None), SERVED).await,
        Ok(())
    );
    // One GET of the list each, and nothing else.
    for lab in [&on_socket, &on_port] {
        let paths: Vec<_> = lab
            .seen()
            .into_iter()
            .map(|seen| (seen.method, seen.path))
            .collect();
        assert_eq!(paths, [("GET".to_owned(), "/v1/models".to_owned())]);
    }
}

#[tokio::test]
async fn a_socket_with_nothing_at_its_path_is_missing() {
    let scratch = Scratch::new("ck");
    let path = scratch.path().join("tunnel.sock");
    assert_eq!(
        probe(&socket_target(path.clone(), None), SERVED).await,
        Err(NotReady::SocketMissing { path })
    );
}

#[tokio::test]
async fn nothing_listening_is_refused_on_a_stale_socket_and_on_a_closed_port() {
    let scratch = Scratch::new("ck");
    // A socket file left behind by a tunnel that died: the file is there, nobody answers.
    let stale = scratch.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).expect("bind"));
    assert_eq!(
        probe(&socket_target(stale, None), SERVED).await,
        Err(NotReady::Refused)
    );
    let closed = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    };
    assert_eq!(
        probe(&port_target(closed, None), SERVED).await,
        Err(NotReady::Refused)
    );
}

#[tokio::test]
async fn an_engine_that_wants_a_key_answers_unauthorized_without_it_and_with_a_wrong_one() {
    let scratch = Scratch::new("ck");
    let lab = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &[SERVED],
        Some("sk-lab-1234"),
    )
    .await;
    assert_eq!(
        probe(&socket_target(lab.socket(), None), SERVED).await,
        Err(NotReady::Unauthorized { status: 401 })
    );
    let wrong = key_file(&scratch, "sk-lab-9999\n", 0o600);
    assert_eq!(
        probe(&socket_target(lab.socket(), Some(wrong)), SERVED).await,
        Err(NotReady::Unauthorized { status: 401 })
    );
    let right = key_file(&scratch, "sk-lab-1234\n", 0o600);
    assert_eq!(
        probe(&socket_target(lab.socket(), Some(right)), SERVED).await,
        Ok(())
    );
    let sent: Vec<_> = lab
        .seen()
        .into_iter()
        .map(|seen| (seen.authorization, seen.status))
        .collect();
    assert_eq!(
        sent,
        [
            (None, 401),
            (Some("Bearer sk-lab-9999".to_owned()), 401),
            (Some("Bearer sk-lab-1234".to_owned()), 200),
        ]
    );
}

#[tokio::test]
async fn a_key_file_that_is_refused_is_never_sent_and_the_engine_is_not_asked() {
    let scratch = Scratch::new("ck");
    let lab = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &[SERVED],
        Some("sk-lab-1234"),
    )
    .await;
    let open = key_file(&scratch, "sk-lab-1234\n", 0o644);
    assert_eq!(
        probe(&socket_target(lab.socket(), Some(open)), SERVED).await,
        Err(NotReady::KeyFile(KeyFileProblem::NotPrivate {
            mode: 0o644
        }))
    );
    assert_eq!(lab.seen(), vec![], "no request, so no token, left");
}

#[tokio::test]
async fn an_engine_that_does_not_serve_the_name_says_what_it_serves() {
    let lab = Lab::start(&Bind::Loopback, "lab", &["llama-3-8b", "qwen3-8b"], None).await;
    assert_eq!(
        probe(&port_target(lab.port(), None), SERVED).await,
        Err(NotReady::ModelAbsent {
            served: vec!["llama-3-8b".to_owned(), "qwen3-8b".to_owned()]
        })
    );
    // It is looked at again each time: the engine changes its model and the next look is ready.
    lab.serve_models(&[SERVED]);
    assert_eq!(probe(&port_target(lab.port(), None), SERVED).await, Ok(()));
}

#[tokio::test]
async fn an_answer_that_is_not_a_model_list_is_unanswered() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi")
                .await;
        }
    });
    assert_eq!(
        probe(&port_target(port, None), SERVED).await,
        Err(NotReady::Unanswered)
    );
    task.abort();
}

#[tokio::test]
async fn an_engine_that_is_slow_to_answer_is_still_ready() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf).await;
            // Longer than the three seconds the look once gave a stage: a loaded computer's
            // engine answers late, and is not therefore away.
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            let body = format!(r#"{{"data":[{{"id":"{SERVED}"}}]}}"#);
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
        }
    });
    assert_eq!(probe(&port_target(port, None), SERVED).await, Ok(()));
    task.abort();
}

#[test]
fn what_is_wrong_is_said_in_a_sentence_and_the_setup_causes_are_the_persons_to_mend() {
    let causes = [
        NotReady::SocketMissing {
            path: "/run/lab.sock".into(),
        },
        NotReady::Refused,
        NotReady::Unauthorized { status: 403 },
        NotReady::ModelAbsent {
            served: vec!["a".into(), "b".into()],
        },
        NotReady::KeyFile(KeyFileProblem::NotYours),
        NotReady::Unanswered,
    ];
    let said: Vec<String> = causes.iter().map(ToString::to_string).collect();
    assert_eq!(said[0], "nothing is at /run/lab.sock: is the tunnel up?");
    assert!(said[3].ends_with("it serves: a, b"));
    let setup: Vec<bool> = causes.iter().map(NotReady::is_setup).collect();
    assert_eq!(setup, [false, false, true, true, true, false]);
}

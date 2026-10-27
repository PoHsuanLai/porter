//! Reading a hello from a computer that answers one.

use porter_tailnet::{Approval, GreetError, Hello, LentModel, Refusal, greet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A computer that reads one request head and answers `response` whole.
async fn answering(response: String) -> TcpStream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });
    TcpStream::connect(at).await.unwrap()
}

fn json(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn a_hello_is_read_with_its_models_and_whether_approval_is_needed() {
    let hello = Hello::new(
        vec![LentModel {
            id: "qwen3-32b".into(),
            name: "Qwen3 32B".into(),
        }],
        Approval::Needed,
    );
    let stream = answering(json("200 OK", &hello.to_json())).await;
    let got = greet(stream, "pi").await.expect("a hello");
    assert_eq!(got, hello);
    assert!(got.needs_approval());
}

#[tokio::test]
async fn a_refusal_is_read_as_the_code_the_computer_gave() {
    let body = r#"{"error":{"message":"any words","type":"refused","code":"waiting"}}"#;
    let stream = answering(json("403 Forbidden", body)).await;
    assert_eq!(
        greet(stream, "pi").await,
        Err(GreetError::Refused(Some(Refusal::Waiting)))
    );
}

#[tokio::test]
async fn a_refusal_with_a_code_this_build_does_not_know_is_still_a_refusal() {
    let body = r#"{"error":{"message":"any words","type":"refused","code":"refused"}}"#;
    let stream = answering(json("403 Forbidden", body)).await;
    assert_eq!(greet(stream, "pi").await, Err(GreetError::Refused(None)));
}

#[tokio::test]
async fn something_else_on_the_port_is_not_a_computer_that_lends() {
    for response in [
        json("200 OK", "<html>hello</html>"),
        json(
            "200 OK",
            r#"{"service":"web","version":1,"models":[],"needs_approval":false}"#,
        ),
        json("404 Not Found", "nothing here"),
    ] {
        let stream = answering(response).await;
        assert_eq!(greet(stream, "pi").await, Err(GreetError::NotLending));
    }
}

#[tokio::test]
async fn a_connection_that_breaks_is_told_so() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        drop(stream);
    });
    let stream = TcpStream::connect(at).await.unwrap();
    assert_eq!(greet(stream, "pi").await, Err(GreetError::Broken));
}

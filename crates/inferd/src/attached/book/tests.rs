use super::*;
use crate::attached::config::{Attached, Place, Reach};
use crate::attached::local_model;
use crate::catalog::parse_entry_text;
use crate::entries;
use crate::lab::Lab;
use crate::startup::Level;
use crate::testkit::Scratch;
use porter_core::ModelId;
use porter_fake_servers::net::Bind;
use std::sync::Mutex;

fn model_on(reach: Reach, place: Place, scratch: &Scratch) -> LocalModel {
    let attached = Attached {
        id: ModelId::parse("tiny-chat").expect("id"),
        reach,
        key_file: None,
        place,
    };
    let entry = parse_entry_text(&entries::chat()).expect("entry");
    local_model(&attached, &[entry], scratch.path()).expect("model")
}

type Lines = Arc<Mutex<Vec<(Level, String)>>>;

fn logged() -> (Log, Lines) {
    let lines = Lines::default();
    let sink = Arc::clone(&lines);
    let log = Log::to(move |level, text| sink.lock().expect("lock").push((level, text.to_owned())));
    (log, lines)
}

#[tokio::test]
async fn an_engine_not_yet_looked_at_is_unavailable_and_a_look_makes_it_ready() {
    let scratch = Scratch::new("bk");
    let lab = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &["tiny-chat"],
        None,
    )
    .await;
    let model = model_on(Reach::Socket(lab.socket()), Place::MyNetwork, &scratch);
    let which = model.model_ref();
    let book = AttachedBook::new(vec![model]);
    assert_eq!(book.state(&which), None);
    assert_eq!(book.readiness(&which), Readiness::Unavailable);
    assert_eq!(book.reprobe(&which).await, Ok(()));
    assert_eq!(book.readiness(&which), Readiness::Ready);
    assert_eq!(lab.seen().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn nothing_looks_between_two_opens_and_each_open_looks_once_per_engine() {
    let scratch = Scratch::new("bk");
    let lab = Lab::start(&Bind::Loopback, "lab", &["tiny-chat"], None).await;
    let model = model_on(
        Reach::Loopback {
            host: std::net::Ipv4Addr::LOCALHOST,
            port: model_http::Port(lab.port()),
        },
        Place::ThisDevice,
        &scratch,
    );
    let book = AttachedBook::new(vec![model]);
    book.reprobe_all().await;
    // A long time passes with no session: no background poll.
    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    assert_eq!(lab.seen().len(), 1);
    book.reprobe_all().await;
    assert_eq!(lab.seen().len(), 2);
}

#[tokio::test]
async fn an_engine_that_goes_away_and_comes_back_is_re_probed_on_the_next_look() {
    let scratch = Scratch::new("bk");
    let lab = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &["tiny-chat"],
        None,
    )
    .await;
    let path = lab.socket();
    let model = model_on(Reach::Socket(path.clone()), Place::MyNetwork, &scratch);
    let which = model.model_ref();
    let (log, lines) = logged();
    let book = AttachedBook::new(vec![model]).logging_to(log);
    assert_eq!(book.reprobe(&which).await, Ok(()));
    // The tunnel drops: the socket is gone.
    drop(lab);
    assert_eq!(
        book.reprobe(&which).await,
        Err(NotReady::SocketMissing { path: path.clone() })
    );
    assert_eq!(book.readiness(&which), Readiness::Unavailable);
    // Asked again while it is still down: the same answer, and no second line in the log.
    assert!(book.reprobe(&which).await.is_err());
    let lab = Lab::start(
        &Bind::Socket(scratch.path().to_path_buf()),
        "lab",
        &["tiny-chat"],
        None,
    )
    .await;
    assert_eq!(lab.socket(), path);
    assert_eq!(book.reprobe(&which).await, Ok(()));
    assert_eq!(book.readiness(&which), Readiness::Ready);
    let lines = lines.lock().expect("lock");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].0, Level::Warn);
    assert!(lines[0].1.contains("attached:tiny-chat"), "{lines:?}");
    assert!(lines[0].1.contains("is the tunnel up?"), "{lines:?}");
}

#[tokio::test]
async fn a_model_the_book_does_not_hold_is_not_ready() {
    let book = AttachedBook::default();
    assert!(book.is_empty());
    let nobody = porter_infer::ModelRef {
        account: porter_core::AccountId::parse("local").expect("id"),
        model: ModelId::parse("nobody").expect("id"),
    };
    assert_eq!(book.reprobe(&nobody).await, Err(NotReady::Unanswered));
    assert_eq!(book.readiness(&nobody), Readiness::Unavailable);
}

//! Listening: when, where, for whom, and what a refusal says.

use super::common::{
    Addresses, addresses, ask, config, connect_from, fake, free_port, listening_at, network,
    observed, wait,
};
use porter_core::{NodeId, UnixSeconds};
use porter_fake::GENEROUS;
use porter_fake_servers::Daemon;
use porter_tailnet::{
    Config, Footing, GuestAnswer, Guests, Lending, Limits, Refusal, Visit, Visits, lend,
};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

/// A handler that records who came and keeps each connection open, answering "pong" to whatever
/// it reads, until the other side closes it.
#[derive(Default)]
struct Recorder {
    came: Mutex<Vec<(NodeId, Footing, IpAddr)>>,
    arrived: Notify,
}

impl Visits for Recorder {
    fn visit(&self, visit: Visit) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.came.lock().unwrap().push((
                visit.welcome().peer().node.clone(),
                visit.welcome().footing(),
                visit.from().ip(),
            ));
            self.arrived.notify_waiters();
            // The permit is kept for as long as the connection is served.
            let (mut stream, _permit) = visit.into_stream();
            let mut buffer = [0_u8; 64];
            while let Ok(n) = stream.read(&mut buffer).await {
                if n == 0 || stream.write_all(b"pong").await.is_err() {
                    break;
                }
            }
        })
    }
}

impl Recorder {
    async fn visits(&self, count: usize) -> Vec<(NodeId, Footing, IpAddr)> {
        let work = async {
            loop {
                let arrived = self.arrived.notified();
                tokio::pin!(arrived);
                arrived.as_mut().enable();
                if self.came.lock().unwrap().len() >= count {
                    return self.came.lock().unwrap().clone();
                }
                arrived.await;
            }
        };
        tokio::time::timeout(GENEROUS, work)
            .await
            .expect("the visits within the generous wait")
    }
}

struct Lender {
    lending: Lending,
    on: watch::Sender<bool>,
    recorder: Arc<Recorder>,
    guests: Arc<Guests>,
    port: u16,
    // Keeps the fake and the observer going.
    _fake: porter_fake_servers::FakeLocalApi,
    _observer: porter_tailnet::Observer,
}

async fn lender(name: &str, a: &Addresses, switched_on: bool, config: Config) -> Lender {
    let (fake, api) = fake(name, Daemon::Running(network(a))).await;
    let (observer, seen) = observed(&api).await;
    let (on, switch) = watch::channel(switched_on);
    let recorder = Arc::new(Recorder::default());
    let guests = Arc::new(Guests::in_memory());
    let port = config.port;
    let lending = lend(
        api,
        seen,
        switch,
        Arc::clone(&guests),
        Arc::clone(&recorder),
        config,
    );
    Lender {
        lending,
        on,
        recorder,
        guests,
        port,
        _fake: fake,
        _observer: observer,
    }
}

fn at(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip, port)
}

#[tokio::test]
async fn dropping_the_listeners_closes_every_one_of_them_and_the_connections_on_them() {
    let a = addresses();
    let l = listening_lender("dropped", &a).await;
    let held = connect_from(a.pi, at(a.me, l.port)).await.expect("connect");
    l.recorder.visits(1).await;
    let (port, _fake, _observer) = (l.port, l._fake, l._observer);
    drop(l.lending);
    // The address refuses, a moment after the listener is let go of, and the connection that
    // was being served is ended with it.
    let deadline = porter_fake::Deadline::generous();
    while listening_at(at(a.me, port)).await {
        if deadline.passed() {
            deadline.fail("the listener to close");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut held = held;
    let mut byte = [0_u8; 1];
    let ended = tokio::time::timeout(GENEROUS, held.read(&mut byte)).await;
    assert!(matches!(ended, Ok(Ok(0) | Err(_))), "{ended:?}");
}

#[tokio::test]
async fn nothing_is_listened_on_until_it_is_switched_on_and_it_stops_when_switched_off() {
    let a = addresses();
    let l = lender("off-by-default", &a, false, config(free_port())).await;
    let mut listening = l.lending.listening();
    // Off: no listener, though Tailscale is up and has given this computer an address.
    assert!(listening.borrow().bound.is_empty());
    assert!(!listening_at(at(a.me, l.port)).await);
    l.on.send(true).unwrap();
    wait(
        &mut listening,
        |now| now.bound.contains(&at(a.me, l.port)),
        "listening on",
    )
    .await;
    assert!(listening_at(at(a.me, l.port)).await);
    l.on.send(false).unwrap();
    wait(
        &mut listening,
        |now| now.bound.is_empty(),
        "listening to stop",
    )
    .await;
    assert!(!listening_at(at(a.me, l.port)).await);
}

#[tokio::test]
async fn it_listens_on_the_networks_addresses_only_and_follows_a_change() {
    let a = addresses();
    let port = free_port();
    let (fake, api) = fake("follows", Daemon::Running(network(&a))).await;
    let (_observer, seen) = observed(&api).await;
    let (_on, switch) = watch::channel(true);
    let lending = lend(
        api,
        seen,
        switch,
        Arc::new(Guests::in_memory()),
        Arc::new(Recorder::default()),
        config(port),
    );
    let mut listening = lending.listening();
    wait(
        &mut listening,
        |now| now.bound == [at(a.me, port)].into(),
        "the first address",
    )
    .await;
    // Tailscale gives this computer another address (and the first one goes).
    fake.edit(|net| net.me.addresses = vec![a.spare.to_string()]);
    wait(
        &mut listening,
        |now| now.bound == [at(a.spare, port)].into(),
        "the new address",
    )
    .await;
    assert!(
        !listening_at(at(a.me, port)).await,
        "the old address is let go"
    );
    assert!(listening_at(at(a.spare, port)).await);
}

#[tokio::test]
async fn never_on_every_address_whatever_tailscale_reports_and_the_config_allows() {
    let a = addresses();
    let port = free_port();
    let (_fake, api) = {
        let mut net = network(&a);
        net.me.addresses = vec!["0.0.0.0".into(), a.me.to_string()];
        fake("not-any", Daemon::Running(net)).await
    };
    let (_observer, seen) = observed(&api).await;
    let (_on, switch) = watch::channel(true);
    // A configuration that allows every address still gets no listener on "any address".
    let everything = Config {
        allowed: |_| true,
        ..config(port)
    };
    let lending = lend(
        api,
        seen,
        switch,
        Arc::new(Guests::in_memory()),
        Arc::new(Recorder::default()),
        everything,
    );
    let mut listening = lending.listening();
    wait(&mut listening, |now| !now.bound.is_empty(), "listening").await;
    assert_eq!(listening.borrow().bound, [at(a.me, port)].into());
}

#[tokio::test]
async fn the_products_configuration_listens_on_the_networks_own_addresses_alone() {
    let product = Config::product();
    assert_eq!(product.port, porter_tailnet::PORT);
    assert!((product.allowed)("100.101.102.103".parse().unwrap()));
    assert!((product.allowed)("fd7a:115c:a1e0::5".parse().unwrap()));
    for not in ["0.0.0.0", "::", "127.0.0.1", "192.168.1.2", "10.0.0.1"] {
        assert!(!(product.allowed)(not.parse().unwrap()), "{not}");
    }
}

#[tokio::test]
async fn an_address_that_cannot_be_listened_on_yet_is_tried_again() {
    let a = addresses();
    let port = free_port();
    // Something else holds the address and port for a while.
    let holder = tokio::net::TcpListener::bind(at(a.me, port)).await.unwrap();
    let l = lender("retry", &a, true, config(port)).await;
    let mut listening = l.lending.listening();
    wait(
        &mut listening,
        |now| now.waiting.contains(&a.me),
        "the address to be waited for",
    )
    .await;
    assert!(listening.borrow().bound.is_empty());
    drop(holder);
    wait(
        &mut listening,
        |now| now.bound.contains(&at(a.me, port)),
        "the retry to succeed",
    )
    .await;
    assert!(listening.borrow().waiting.is_empty());
}

#[tokio::test]
async fn tailscale_going_away_closes_the_listeners_and_coming_back_opens_them() {
    let a = addresses();
    let l = lender("goes-away", &a, true, config(free_port())).await;
    let mut listening = l.lending.listening();
    wait(&mut listening, |now| !now.bound.is_empty(), "listening").await;
    l._fake.stop();
    wait(
        &mut listening,
        |now| now.bound.is_empty(),
        "listening to stop",
    )
    .await;
    l._fake.restart().await.unwrap();
    wait(
        &mut listening,
        |now| !now.bound.is_empty(),
        "listening again",
    )
    .await;
}

/// The whole answer to a request sent from `source`.
async fn answer_to(l: &Lender, a: &Addresses, source: IpAddr) -> String {
    let stream = connect_from(source, at(a.me, l.port))
        .await
        .expect("connect");
    ask(stream, "GET /hello HTTP/1.1\r\nHost: x\r\n\r\n").await
}

async fn listening_lender(name: &str, a: &Addresses) -> Lender {
    let l = lender(name, a, true, config(free_port())).await;
    let mut listening = l.lending.listening();
    wait(&mut listening, |now| !now.bound.is_empty(), "listening").await;
    l
}

#[tokio::test]
async fn one_of_the_persons_own_computers_gets_through_as_new_and_a_yes_makes_it_approved() {
    let a = addresses();
    let l = listening_lender("through", &a).await;
    let stream = connect_from(a.pi, at(a.me, l.port)).await.expect("connect");
    let mut stream = stream;
    stream.write_all(b"ping").await.unwrap();
    let mut pong = [0_u8; 4];
    tokio::time::timeout(GENEROUS, stream.read_exact(&mut pong))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&pong, b"pong");
    let came = l.recorder.visits(1).await;
    assert_eq!(came[0].0.as_str(), "nPI");
    assert_eq!(came[0].1, Footing::New);
    assert_eq!(came[0].2, a.pi);
    l.guests
        .set(
            &NodeId::parse("nPI").unwrap(),
            "pi",
            GuestAnswer::Allow,
            UnixSeconds(1),
        )
        .unwrap();
    let again = connect_from(a.pi, at(a.me, l.port)).await.expect("connect");
    drop(again);
    assert_eq!(l.recorder.visits(2).await[1].1, Footing::Approved);
}

#[tokio::test]
async fn a_tagged_server_another_persons_computer_and_a_stranger_are_refused_with_words() {
    let a = addresses();
    let l = listening_lender("refused", &a).await;
    // Tagged (build-box at `other`), shared in (friends-pc at `spare`).
    let tagged = answer_to(&l, &a, a.other).await;
    let shared = answer_to(&l, &a, a.spare).await;
    // An address Tailscale has no computer for.
    let stranger = answer_to(&l, &a, "127.200.200.200".parse().unwrap()).await;
    for (answer, refusal) in [
        (tagged, Refusal::Tagged),
        (shared, Refusal::Shared),
        (stranger, Refusal::Unknown),
    ] {
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden"), "{answer}");
        assert!(answer.contains(&refusal.to_string()), "{answer}");
    }
    assert!(
        l.recorder.came.lock().unwrap().is_empty(),
        "none of them reached the handler"
    );
}

#[tokio::test]
async fn a_request_from_this_computers_own_address_is_refused() {
    // A program of another account on this computer reaches the listener through the network
    // address; it must not pass as one of the person's other computers.
    let a = addresses();
    let l = listening_lender("own-address", &a).await;
    let answer = answer_to(&l, &a, a.me).await;
    assert!(answer.starts_with("HTTP/1.1 403 Forbidden"), "{answer}");
    assert!(
        answer.contains(&Refusal::ThisComputer.to_string()),
        "{answer}"
    );
    assert!(l.recorder.came.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_no_from_the_person_is_a_refusal_and_a_yes_lets_a_shared_computer_in() {
    let a = addresses();
    let l = listening_lender("answers", &a).await;
    let now = UnixSeconds(1);
    l.guests
        .set(&NodeId::parse("nPI").unwrap(), "pi", GuestAnswer::Deny, now)
        .unwrap();
    l.guests
        .set(
            &NodeId::parse("nFRIEND").unwrap(),
            "friends-pc",
            GuestAnswer::Allow,
            now,
        )
        .unwrap();
    let denied = answer_to(&l, &a, a.pi).await;
    assert!(denied.contains(&Refusal::Denied.to_string()), "{denied}");
    let stream = connect_from(a.spare, at(a.me, l.port))
        .await
        .expect("connect");
    drop(stream);
    let came = l.recorder.visits(1).await;
    assert_eq!(came[0].0.as_str(), "nFRIEND");
    assert_eq!(came[0].1, Footing::Approved);
}

#[tokio::test]
async fn when_tailscale_cannot_say_who_asks_nobody_is_let_in() {
    let a = addresses();
    let l = listening_lender("cannot-check", &a).await;
    // Tailscale is gone for the whois, but the observer still has the identity it last saw.
    l._fake.stop();
    let answer = answer_to(&l, &a, a.pi).await;
    // Either the connection was judged with no answer to the question, or this computer had
    // already noticed Tailscale going: in both cases no handler was reached.
    assert!(
        answer.contains(&Refusal::CouldNotCheck.to_string())
            || answer.contains(&Refusal::Off.to_string())
            || answer.is_empty(),
        "{answer}"
    );
    assert!(l.recorder.came.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_computer_has_a_few_connections_at_once_and_no_more() {
    let a = addresses();
    let port = free_port();
    let limits = Limits {
        per_guest: 2,
        in_all: 8,
    };
    let l = lender(
        "limits",
        &a,
        true,
        Config {
            limits,
            ..config(port)
        },
    )
    .await;
    let mut listening = l.lending.listening();
    wait(&mut listening, |now| !now.bound.is_empty(), "listening").await;
    let first = connect_from(a.pi, at(a.me, port)).await.unwrap();
    let second = connect_from(a.pi, at(a.me, port)).await.unwrap();
    l.recorder.visits(2).await;
    let third = connect_from(a.pi, at(a.me, port)).await.unwrap();
    let answer = ask(third, "GET /hello HTTP/1.1\r\n\r\n").await;
    assert!(answer.starts_with("HTTP/1.1 429"), "{answer}");
    assert!(answer.contains(&Refusal::Busy.to_string()), "{answer}");
    // Closing one makes room.
    drop(first);
    let mut room = false;
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() && !room {
        let mut again = connect_from(a.pi, at(a.me, port)).await.unwrap();
        again.write_all(b"ping").await.unwrap();
        let mut head = [0_u8; 4];
        room = matches!(
            tokio::time::timeout(GENEROUS, again.read_exact(&mut head)).await,
            Ok(Ok(_))
        ) && &head == b"pong";
        if !room {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    assert!(room, "a connection closed makes room for another");
    drop(second);
}

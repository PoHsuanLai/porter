use super::*;
use crate::guests::{AskOutcome, GuestAnswer, GuestEvent};
use porter_core::UnixSeconds;
use porter_tailscale::{User, UserId, WhoIsNode};
use std::sync::{Arc, Mutex};

const NOW: UnixSeconds = UnixSeconds(1_790_000_000);

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a node id")
}

fn ip(text: &str) -> IpAddr {
    text.parse().expect("an address")
}

/// This computer: desk, user 1, one IPv4 and one IPv6 address.
fn me() -> Identity {
    Identity::new(
        node("nSELF"),
        UserId(1),
        "desk".into(),
        vec![ip("100.64.0.1"), ip("fd7a:115c:a1e0::1")],
    )
}

/// What Tailscale says of the computer at `address`.
fn who(id: &str, dns: &str, user: i64, address: &str) -> WhoIs {
    WhoIs {
        node: WhoIsNode {
            id: node(id),
            dns: dns.into(),
            user: UserId(user),
            sharer: None,
            tags: Vec::new(),
            addresses: vec![ip(address)],
        },
        user: User {
            id: UserId(user),
            login: format!("user{user}@example.org"),
            display_name: String::new(),
        },
    }
}

fn pi() -> WhoIs {
    who("nPI", "pi.tail1234.ts.net", 1, "100.64.0.2")
}

fn judged(
    from: &str,
    answer: Result<WhoIs, TailscaleError>,
    guests: &Guests,
) -> Result<Welcome, Refusal> {
    judge(&me(), ip(from), answer, guests)
}

#[test]
fn a_computer_of_the_same_user_that_nobody_answered_about_is_let_through_as_new() {
    let welcome = judged("100.64.0.2", Ok(pi()), &Guests::in_memory()).expect("welcome");
    assert_eq!(welcome.footing(), Footing::New);
    assert_eq!(welcome.peer().node, node("nPI"));
    assert_eq!(welcome.peer().name, "pi");
    assert_eq!(welcome.peer().owner, MachineOwner::Mine);
}

#[test]
fn a_request_from_this_computers_own_address_is_refused_whoever_tailscale_says_it_is() {
    // Rule (c): a program of another account on this computer reaches the listener through the
    // network address, and Tailscale then calls that "this computer", of the same user.
    let guests = Guests::in_memory();
    guests
        .set(&node("nSELF"), "desk", GuestAnswer::Allow, NOW)
        .unwrap();
    for own in ["100.64.0.1", "fd7a:115c:a1e0::1"] {
        for answer in [
            Ok(who("nSELF", "desk.tail1234.ts.net", 1, own)),
            // Even if Tailscale were to name another computer for the address.
            Ok(who("nPI", "pi.tail1234.ts.net", 1, own)),
            Err(TailscaleError::NoSuchPeer),
            Err(TailscaleError::NotRunning),
        ] {
            assert_eq!(
                judged(own, answer, &guests).unwrap_err(),
                Refusal::ThisComputer,
                "{own}"
            );
        }
    }
}

#[test]
fn the_same_computer_through_another_address_is_still_this_computer() {
    let answer = Ok(who("nSELF", "desk.tail1234.ts.net", 1, "100.64.0.9"));
    assert_eq!(
        judged("100.64.0.9", answer, &Guests::in_memory()).unwrap_err(),
        Refusal::ThisComputer
    );
}

#[test]
fn an_address_tailscale_does_not_know_or_cannot_be_asked_about_is_refused() {
    let guests = Guests::in_memory();
    assert_eq!(
        judged("100.64.0.2", Err(TailscaleError::NoSuchPeer), &guests).unwrap_err(),
        Refusal::Unknown
    );
    for failure in [
        TailscaleError::NotRunning,
        TailscaleError::NotInstalled,
        TailscaleError::Refused,
        TailscaleError::SignedOut,
        TailscaleError::Malformed,
        TailscaleError::TimedOut,
    ] {
        assert_eq!(
            judged("100.64.0.2", Err(failure), &guests).unwrap_err(),
            Refusal::CouldNotCheck,
            "{failure}"
        );
    }
}

#[test]
fn an_answer_that_does_not_list_the_address_is_not_believed() {
    let other = who("nPI", "pi.tail1234.ts.net", 1, "100.64.0.77");
    assert_eq!(
        judged("100.64.0.2", Ok(other), &Guests::in_memory()).unwrap_err(),
        Refusal::Unknown
    );
}

#[test]
fn a_tagged_server_another_persons_and_a_shared_computer_are_refused() {
    let guests = Guests::in_memory();
    let mut tagged = who("nBUILD", "build-box.tail1234.ts.net", 1, "100.64.0.4");
    tagged.node.tags.push("tag:ci".into());
    assert_eq!(
        judged("100.64.0.4", Ok(tagged), &guests).unwrap_err(),
        Refusal::Tagged
    );
    let friend = who("nFRIEND", "friends-pc.tail1234.ts.net", 2, "100.64.0.5");
    assert_eq!(
        judged("100.64.0.5", Ok(friend), &guests).unwrap_err(),
        Refusal::Shared
    );
    let mut shared_in = who("nSHARED", "shared.tail1234.ts.net", 1, "100.64.0.6");
    shared_in.node.sharer = Some(UserId(2));
    assert_eq!(
        judged("100.64.0.6", Ok(shared_in), &guests).unwrap_err(),
        Refusal::Shared
    );
}

#[test]
fn the_persons_word_ahead_of_time_lets_in_a_computer_that_would_be_refused() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nFRIEND"), "friends-pc", GuestAnswer::Allow, NOW)
        .unwrap();
    guests
        .set(&node("nBUILD"), "build-box", GuestAnswer::Allow, NOW)
        .unwrap();
    let friend = who("nFRIEND", "friends-pc.tail1234.ts.net", 2, "100.64.0.5");
    let welcome = judged("100.64.0.5", Ok(friend), &guests).expect("welcome");
    assert_eq!(welcome.footing(), Footing::Approved);
    assert_eq!(welcome.peer().owner, MachineOwner::Shared);
    let mut tagged = who("nBUILD", "build-box.tail1234.ts.net", 1, "100.64.0.4");
    tagged.node.tags.push("tag:ci".into());
    assert_eq!(
        judged("100.64.0.4", Ok(tagged), &guests)
            .expect("welcome")
            .footing(),
        Footing::Approved
    );
}

#[test]
fn a_no_beats_everything_and_a_yes_is_by_id_not_by_name() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Deny, NOW)
        .unwrap();
    assert_eq!(
        judged("100.64.0.2", Ok(pi()), &guests).unwrap_err(),
        Refusal::Denied
    );
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Allow, NOW)
        .unwrap();
    // Renamed on the network: still the one the person said yes to.
    let renamed = who("nPI", "kitchen-pi.tail1234.ts.net", 1, "100.64.0.2");
    assert_eq!(
        judged("100.64.0.2", Ok(renamed), &guests)
            .expect("welcome")
            .footing(),
        Footing::Approved
    );
    // Another computer with the old name and the old address is not.
    let impostor = who("nOTHER", "pi.tail1234.ts.net", 1, "100.64.0.2");
    assert_eq!(
        judged("100.64.0.2", Ok(impostor), &guests)
            .expect("welcome")
            .footing(),
        Footing::New
    );
}

#[test]
fn a_new_computer_is_asked_about_once_and_refused_until_the_person_answers() {
    let guests = Guests::in_memory();
    let told = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&told);
    guests.on_event(move |event| log.lock().unwrap().push(event.clone()));
    let welcome = judged("100.64.0.2", Ok(pi()), &guests).expect("welcome");
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::Waiting));
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::Waiting));
    let asked = told
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, GuestEvent::Asked(_)))
        .count();
    assert_eq!(asked, 1, "the person is asked once");
    guests
        .answer(&node("nPI"), GuestAnswer::Allow, NOW)
        .unwrap();
    assert_eq!(welcome.admit(&guests, NOW), Ok(()));
}

#[test]
fn a_yes_taken_back_is_taken_back_for_the_next_request() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Allow, NOW)
        .unwrap();
    let welcome = judged("100.64.0.2", Ok(pi()), &guests).expect("welcome");
    assert_eq!(welcome.admit(&guests, NOW), Ok(()));
    guests
        .set(&node("nPI"), "pi", GuestAnswer::Deny, NOW)
        .unwrap();
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::Denied));
    guests.forget(&node("nPI")).unwrap();
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::Waiting));
}

#[test]
fn someone_elses_computer_is_never_asked_about() {
    let guests = Guests::in_memory();
    guests
        .set(&node("nFRIEND"), "friends-pc", GuestAnswer::Allow, NOW)
        .unwrap();
    let friend = who("nFRIEND", "friends-pc.tail1234.ts.net", 2, "100.64.0.5");
    let welcome = judged("100.64.0.5", Ok(friend), &guests).expect("welcome");
    guests.forget(&node("nFRIEND")).unwrap();
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::Shared));
    assert!(guests.asking().is_empty(), "no question is raised for it");
    assert_eq!(guests.ask(&node("nPI"), "pi", NOW), Ok(AskOutcome::Raised));
}

#[test]
fn too_many_waiting_is_a_refusal_with_words() {
    let guests = Guests::in_memory();
    for n in 0..16 {
        guests.ask(&node(&format!("n{n}")), "x", NOW).unwrap();
    }
    let welcome = judged("100.64.0.2", Ok(pi()), &guests).expect("welcome");
    assert_eq!(welcome.admit(&guests, NOW), Err(Refusal::TooManyAsking));
}

//! SRV records, ported from `srv_records` in mailo's `crates/mail-proto/tests/discover.rs`.

use super::*;
use crate::found::Source;
use crate::testing::{ADDRESS, Records, name, srv};

fn urls(found: &Found) -> Vec<String> {
    found.endpoints.iter().map(|e| e.url.to_string()).collect()
}

#[test]
fn the_lowest_priority_then_heaviest_target_is_chosen() {
    let answers = SrvAnswers {
        imaps: vec![
            srv(20, 0, 993, "backup.example.test."),
            srv(10, 5, 993, "light.example.test."),
            srv(10, 50, 9993, "heavy.example.test."),
        ],
        submissions: vec![srv(0, 0, 465, "smtp.example.test.")],
        ..SrvAnswers::default()
    };
    let found = found_from_srv(ADDRESS, &answers).expect("found");
    assert_eq!(
        urls(&found),
        [
            "imaps://heavy.example.test:9993",
            "smtps://smtp.example.test:465"
        ]
    );
    assert_eq!(found.source, Source::Srv);
    assert!(found.endpoints.iter().all(|e| e.login.0 == ADDRESS));
}

#[test]
fn equal_priority_and_weight_fall_back_to_the_name() {
    let answers = SrvAnswers {
        imaps: vec![
            srv(0, 1, 993, "b.example.test"),
            srv(0, 1, 993, "a.example.test"),
        ],
        submissions: vec![srv(0, 0, 465, "s.example.test")],
        ..SrvAnswers::default()
    };
    let found = found_from_srv(ADDRESS, &answers).expect("found");
    assert_eq!(
        found.endpoints[0].url.to_string(),
        "imaps://a.example.test:993"
    );
}

#[test]
fn the_starttls_names_serve_when_the_implicit_ones_are_absent() {
    let answers = SrvAnswers {
        imap: vec![srv(0, 0, 143, "imap.example.test")],
        submission: vec![srv(0, 0, 587, "submit.example.test")],
        ..SrvAnswers::default()
    };
    let found = found_from_srv(ADDRESS, &answers).expect("found");
    assert_eq!(
        urls(&found),
        [
            "imap://imap.example.test:143",
            "smtp://submit.example.test:587"
        ]
    );
    assert!(found.endpoints.iter().all(|e| e.tls == Tls::StartTls));
}

#[test]
fn implicit_names_win_over_starttls_ones() {
    let answers = SrvAnswers {
        imaps: vec![srv(5, 0, 993, "tls.example.test")],
        imap: vec![srv(0, 0, 143, "plain.example.test")],
        submissions: vec![srv(5, 0, 465, "tls.example.test")],
        submission: vec![srv(0, 0, 587, "plain.example.test")],
    };
    let found = found_from_srv(ADDRESS, &answers).expect("found");
    assert!(found.endpoints.iter().all(|e| e.tls == Tls::Implicit));
}

#[test]
fn a_dot_target_or_port_zero_is_not_offered() {
    // A target of "." does not parse as a name, so a record with it never reaches here; port 0
    // is the other spelling of "not offered".
    let answers = SrvAnswers {
        imaps: vec![srv(0, 0, 0, "dead.example.test")],
        submissions: vec![srv(0, 0, 465, "s.example.test")],
        ..SrvAnswers::default()
    };
    assert_eq!(
        found_from_srv(ADDRESS, &answers),
        Err(DiscoverFault::NoServers)
    );
    assert!(DomainName::parse(".").is_err());
}

#[test]
fn a_missing_side_is_no_servers() {
    let only_imap = SrvAnswers {
        imaps: vec![srv(0, 0, 993, "imap.example.test")],
        ..SrvAnswers::default()
    };
    assert_eq!(
        found_from_srv(ADDRESS, &only_imap),
        Err(DiscoverFault::NoServers)
    );
    let only_smtp = SrvAnswers {
        submissions: vec![srv(0, 0, 465, "s.example.test")],
        ..SrvAnswers::default()
    };
    assert_eq!(
        found_from_srv(ADDRESS, &only_smtp),
        Err(DiscoverFault::NoServers)
    );
}

#[test]
fn the_four_names_are_asked() {
    assert_eq!(
        srv_names(&name("example.test")),
        [
            "_imaps._tcp.example.test",
            "_imap._tcp.example.test",
            "_submissions._tcp.example.test",
            "_submission._tcp.example.test"
        ]
    );
}

#[tokio::test]
async fn lookup_collects_what_each_name_answered() {
    let mut dns = Records::default();
    dns.srv.insert(
        "_imaps._tcp.example.test".to_owned(),
        vec![srv(0, 1, 993, "imap.example.test")],
    );
    dns.srv.insert(
        "_submissions._tcp.example.test".to_owned(),
        vec![srv(0, 1, 465, "smtp.example.test")],
    );
    let answers = lookup_srv(&dns, &name("example.test"))
        .await
        .expect("reached");
    assert_eq!(answers.imaps.len(), 1);
    assert!(answers.imap.is_empty());
    assert_eq!(answers.submissions.len(), 1);
    assert_eq!(dns.asked.lock().expect("asked").len(), 4);
}

#[tokio::test]
async fn lookup_fails_only_when_nothing_could_be_asked() {
    let dns = Records {
        down: true,
        ..Records::default()
    };
    assert_eq!(
        lookup_srv(&dns, &name("example.test")).await,
        Err(DnsFault::Unreachable)
    );
    let empty = lookup_srv(&Records::default(), &name("example.test")).await;
    assert_eq!(empty, Ok(SrvAnswers::default()));
}

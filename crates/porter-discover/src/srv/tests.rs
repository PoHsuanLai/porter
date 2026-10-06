//! SRV records, ported from `srv_records` in mailo's `crates/mail-proto/tests/discover.rs`.

use super::*;
use crate::found::{Pop3, Pop3Server, Source};
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
        pop3s: Vec::new(),
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
fn the_five_names_are_asked() {
    assert_eq!(
        srv_names(&name("example.test")),
        [
            "_imaps._tcp.example.test",
            "_imap._tcp.example.test",
            "_pop3s._tcp.example.test",
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
    assert_eq!(dns.asked.lock().expect("asked").len(), 5);
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

fn pop3_server(host: &str, port: u16) -> Pop3Server {
    Pop3Server {
        host: host.to_owned(),
        port,
        tls: Tls::Implicit,
        login: LoginName(ADDRESS.to_owned()),
    }
}

fn pop3_only() -> SrvAnswers {
    SrvAnswers {
        pop3s: vec![
            srv(10, 0, 995, "backup.example.test"),
            srv(0, 0, 0, "dead.example.test"),
            srv(0, 5, 995, "pop.example.test"),
        ],
        submissions: vec![srv(0, 0, 465, "smtp.example.test")],
        ..SrvAnswers::default()
    }
}

#[test]
fn pop3_only_srv_is_a_finding_under_report_with_the_smtp_endpoint_alone() {
    let found = found_from_srv_with(ADDRESS, &pop3_only(), Pop3::Report).expect("found");
    assert_eq!(urls(&found), ["smtps://smtp.example.test:465"]);
    assert!(found.claims.is_empty());
    assert_eq!(found.source, Source::Srv);
    assert_eq!(
        found.pop3,
        [
            pop3_server("pop.example.test", 995),
            pop3_server("backup.example.test", 995)
        ]
    );
}

#[test]
fn pop3_only_srv_is_no_servers_under_ignore_and_by_default() {
    assert_eq!(
        found_from_srv_with(ADDRESS, &pop3_only(), Pop3::Ignore),
        Err(DiscoverFault::NoServers)
    );
    assert_eq!(
        found_from_srv(ADDRESS, &pop3_only()),
        Err(DiscoverFault::NoServers)
    );
}

#[test]
fn pop3_beside_imap_is_listed_only_under_report_and_the_endpoints_do_not_change() {
    let mut answers = pop3_only();
    answers.imaps = vec![srv(0, 0, 993, "imap.example.test")];
    let reported = found_from_srv_with(ADDRESS, &answers, Pop3::Report).expect("found");
    let ignored = found_from_srv_with(ADDRESS, &answers, Pop3::Ignore).expect("found");
    assert_eq!(reported.pop3.len(), 2);
    assert!(ignored.pop3.is_empty());
    assert_eq!(reported.endpoints, ignored.endpoints);
    assert_eq!(reported.claims, ignored.claims);
}

#[test]
fn pop3_without_submission_or_with_only_port_zero_is_still_no_servers() {
    let mut answers = pop3_only();
    answers.submissions.clear();
    assert_eq!(
        found_from_srv_with(ADDRESS, &answers, Pop3::Report),
        Err(DiscoverFault::NoServers)
    );
    let dead = SrvAnswers {
        pop3s: vec![srv(0, 0, 0, "dead.example.test")],
        submissions: vec![srv(0, 0, 465, "s.example.test")],
        ..SrvAnswers::default()
    };
    assert_eq!(
        found_from_srv_with(ADDRESS, &dead, Pop3::Report),
        Err(DiscoverFault::NoServers)
    );
}

#[tokio::test]
async fn lookup_collects_the_pop3s_answer() {
    let mut dns = Records::default();
    dns.srv.insert(
        "_pop3s._tcp.example.test".to_owned(),
        vec![srv(0, 1, 995, "pop.example.test")],
    );
    let answers = lookup_srv(&dns, &name("example.test"))
        .await
        .expect("reached");
    assert_eq!(answers.pop3s.len(), 1);
}

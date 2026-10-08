//! The ManageSieve relay against a minimal fake in the tests.

use crate::common;

use common::sieve::FakeSieve;
use common::*;
use porter_core::{Family, Tls};
use porter_proxy::{RelayEnd, RelayFault};

fn sieve_plan(port: u16, auth: porter_core::RelayAuth) -> porter_core::RelayPlan {
    plan(
        Family::Imap,
        &format!("sieve://127.0.0.1:{port}"),
        Tls::StartTls,
        auth,
    )
}

#[tokio::test]
async fn the_app_sees_capabilities_without_sasl_and_starttls_then_a_working_session() {
    let fake = FakeSieve::start().await;
    let mut app = start(
        sieve_plan(port(&fake.address), password()),
        trusting_fakes(),
    );
    let greeting = app.read_until("OK \"porter relay ready\"\r\n").await;
    assert!(
        greeting.contains("\"SIEVE\" \"fileinto vacation\""),
        "{greeting}"
    );
    assert!(
        !greeting.contains("SASL") && !greeting.contains("STARTTLS"),
        "{greeting}"
    );
    app.send("LISTSCRIPTS\r\n").await;
    let scripts = app.read_until("Listscripts completed.\"\r\n").await;
    assert!(scripts.contains("\"main\" ACTIVE"), "{scripts}");
    app.send("LOGOUT\r\n").await;
    app.read_until("OK \"bye\"\r\n").await;
    assert!(!app.everything().contains(PASSWORD));
    app.finish().await;
    let logins = fake.logins();
    assert_eq!(logins.len(), 1);
    assert!(logins[0].accepted && logins[0].secure);
    assert_eq!(logins[0].password, PASSWORD);
}

#[tokio::test]
async fn a_wrong_password_is_refused_and_the_app_sees_nothing() {
    let fake = FakeSieve::start().await;
    let wrong = porter_core::RelayAuth::Password(porter_core::SecretText::new("nope"));
    let mut app = start(sieve_plan(port(&fake.address), wrong), trusting_fakes());
    assert_eq!(app.read_to_end().await, "");
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Refused));
    assert_eq!(fake.logins().len(), 1);
}

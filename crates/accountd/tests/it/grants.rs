//! `org.quire.Grants1`: an app finds the accounts it holds a grant for, is asked through the
//! sheet host (once for an account: the answer is kept), and opens a stream to the account's server that accountd has signed in: the
//! descriptor reaches a fake DAV server, which saw accountd's credential and the app never did.
//! Nobody asks about another app's grants, and only a porter daemon, Settings or the shell lists
//! the accounts.

use crate::common;
use crate::launcher_register::error_name;

use accountd::{Options, Relays};
use common::*;
use porter_callers::CallerRole;
use porter_capability::{Access, Delta, StorageScope};
use porter_core::consent::{ConsentAnswer, Grant, GrantScope};
use porter_core::{
    Account, AccountId, EndpointUrl, Family, LoginName, ProviderId, ServiceEndpoint,
};
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{
    ACCOUNT_OPTION, GrantsProxy, ServiceRoot, Sheet, candidate_from_dbus, from_text, to_text,
};
use porter_fake::cloud_provider;
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_match::{Need, StorageNeed};
use porter_proxy::RustlsConnect;
use porter_sheet::SheetInput;
use porter_wire::AccountsReply;
use std::io::{Read, Write};
use zbus::zvariant::{OwnedValue, Value};

const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const UNKNOWN_GRANT: &str = "org.quire.Accounts1.Error.UnknownGrant";
const ENDPOINT_NOT_GRANTED: &str = "org.quire.Accounts1.Error.EndpointNotGranted";

fn need() -> Need {
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ))
}

/// `fake-storage` with one WebDAV server: the fake DAV server on this computer.
fn dav_account(dav: &Running<DavHandle>) -> Account {
    let url = EndpointUrl::parse(&format!("{}/dav/files/", dav.base_url())).expect("url");
    let endpoint = ServiceEndpoint::coherent(Family::WebDav, url, &LoginName("alice".into()))
        .expect("a coherent endpoint");
    Account {
        endpoints: vec![endpoint],
        ..storage_account()
    }
}

/// accountd over the DAV account, a person who allows the first account asked about, and relays
/// that trust the fakes' scratch CA.
async fn rig(dav: &Running<DavHandle>) -> Rig {
    let account = dav_account(dav);
    let allowed = account.id.clone();
    rig_of(vec![account], allowed).await
}

/// A second account that fits the same need, on the same server.
fn second_account(dav: &Running<DavHandle>) -> Account {
    Account {
        id: AccountId::parse("second-storage").expect("id"),
        ..dav_account(dav)
    }
}

/// accountd over `accounts`, a person who allows `allowed` whatever the sheet is about.
async fn rig_of(accounts: Vec<Account>, allowed: AccountId) -> Rig {
    let host = SheetHost::answering(SheetInput::Answer(ConsentAnswer::Allow {
        account: allowed,
        scope: GrantScope::Always,
    }));
    let options = Options {
        relays: Some(Relays::with_connector(RustlsConnect::trusting([
            porter_fake_servers::tls::ca_der(),
        ]))),
        ..Options::default()
    };
    Rig::start_holding(options, host, vec![cloud_provider()], accounts).await
}

async fn grants_of(connection: &zbus::Connection) -> GrantsProxy<'static> {
    GrantsProxy::new(connection).await.expect("proxy")
}

/// `RequestGrant` as an app does it (for the account named, or any when `account` is empty), and
/// the sheet's answer.
async fn request(
    connection: &zbus::Connection,
    proxy: &GrantsProxy<'_>,
    account: &str,
) -> Result<AccountsReply, zbus::Error> {
    let mut sheet = Sheet::subscribe_on(connection, ServiceRoot::Accounts)
        .await
        .expect("subscribe");
    let mut options = sheet.options();
    if !account.is_empty() {
        let named = OwnedValue::try_from(Value::from(account.to_owned()))?;
        options.insert(ACCOUNT_OPTION.to_owned(), named);
    }
    let path = proxy
        .request_grant(&to_text(&need()), "photos", "interactive", "", &options)
        .await?;
    let (code, results) = sheet.response(&path).await.expect("response");
    Ok(reply_of(SheetKind::Choose, code, results).expect("a reply"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_is_asked_once_and_then_reaches_the_server_through_a_descriptor() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let rig = rig(&dav).await;
    let connection = rig.client("org.quire.Photos").await;
    let proxy = grants_of(&connection).await;
    let (need_text, class, usage) = (to_text(&need()), "photos", "interactive");

    // Nothing is granted yet, and the app learns only that it may ask.
    assert!(
        proxy
            .candidates(&need_text, class, usage)
            .await
            .expect("candidates")
            .is_empty()
    );
    assert_eq!(
        proxy
            .availability(&need_text, class, usage)
            .await
            .expect("availability"),
        "available_needs_consent"
    );

    let AccountsReply::Chosen(chosen) = request(&connection, &proxy, "").await.expect("asked")
    else {
        panic!("the person allowed it");
    };
    assert_eq!(chosen.account, dav_account(&dav).id);
    assert_eq!(
        rig.host_log.calls().opened.len(),
        1,
        "one sheet, and its answer is kept"
    );

    let found: Vec<_> = proxy
        .candidates(&need_text, class, usage)
        .await
        .expect("candidates")
        .into_iter()
        .map(|row| candidate_from_dbus(row).expect("a candidate"))
        .collect();
    assert_eq!(found, std::slice::from_ref(&chosen));
    let held: Vec<Grant> = from_text(&proxy.grants().await.expect("grants")).expect("grants");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, chosen.grant);
    assert_eq!(held[0].key.app, app("org.quire.Photos"));

    // The stream: a Unix socket whose other end accountd's relay holds.
    let endpoint = chosen.endpoints[0].url.clone();
    let fd = proxy
        .open_authenticated(chosen.grant.as_str(), endpoint.as_str())
        .await
        .expect("a descriptor");
    let mut stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    let host = dav.base_url().trim_start_matches("http://").to_owned();
    let asked = format!(
        "PROPFIND /dav/files/ HTTP/1.1\r\nHost: {host}\r\nDepth: 0\r\nContent-Length: 0\r\n\r\n"
    );
    let answer = tokio::task::spawn_blocking(move || {
        stream.write_all(asked.as_bytes()).expect("write");
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).expect("read") == 0 {
                break;
            }
            head.push(byte[0]);
        }
        String::from_utf8_lossy(&head).into_owned()
    })
    .await
    .expect("join");
    assert!(answer.starts_with("HTTP/1.1 207"), "{answer}");
    assert!(!answer.contains(APP_PASSWORD), "{answer}");
    let hits = dav.hits();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0]
            .authorization
            .as_deref()
            .is_some_and(|authorization| authorization.starts_with("Basic ")),
        "the server saw accountd's credential"
    );

    // Revoking is the app's own, once.
    proxy.revoke(chosen.grant.as_str()).await.expect("revoked");
    assert!(
        proxy
            .candidates(&need_text, class, usage)
            .await
            .expect("candidates")
            .is_empty()
    );
    let again = proxy
        .revoke(chosen.grant.as_str())
        .await
        .expect_err("already gone");
    assert_eq!(error_name(again), UNKNOWN_GRANT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_is_the_apps_own_and_an_endpoint_outside_it_is_refused() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let rig = rig(&dav).await;
    let photos = rig.client("org.quire.Photos").await;
    let proxy = grants_of(&photos).await;
    let AccountsReply::Chosen(chosen) = request(&photos, &proxy, "").await.expect("asked") else {
        panic!("the person allowed it");
    };
    let endpoint = chosen.endpoints[0].url.clone();

    // Another app holds none of it: it cannot use, read or revoke Photos' grant.
    let files = rig.client("org.quire.Files").await;
    let other = grants_of(&files).await;
    let held: Vec<Grant> = from_text(&other.grants().await.expect("grants")).expect("grants");
    assert!(held.is_empty());
    for failed in [
        other
            .open_authenticated(chosen.grant.as_str(), endpoint.as_str())
            .await
            .map(drop),
        other.revoke(chosen.grant.as_str()).await,
    ] {
        assert_eq!(error_name(failed.expect_err("refused")), UNKNOWN_GRANT);
    }

    // The holder, at a server the account has not got.
    let elsewhere = "https://elsewhere.invalid/dav/";
    let refused = proxy
        .open_authenticated(chosen.grant.as_str(), elsewhere)
        .await
        .expect_err("not one of the account's");
    assert_eq!(error_name(refused), ENDPOINT_NOT_GRANTED);

    // Words and texts that are none are refused as bad arguments, not guessed.
    for bad in [
        proxy
            .candidates("not json", "photos", "interactive")
            .await
            .map(drop),
        proxy
            .candidates(&to_text(&need()), "not a class", "interactive")
            .await
            .map(drop),
        proxy
            .availability(&to_text(&need()), "photos", "not a usage")
            .await
            .map(drop),
        proxy.revoke("Not A Grant").await,
        proxy
            .open_authenticated("Not A Grant", elsewhere)
            .await
            .map(drop),
        proxy
            .open_linked(chosen.grant.as_str(), "not a url")
            .await
            .map(drop),
        proxy
            .asks_on_use("Not A Provider", "storage")
            .await
            .map(drop),
        proxy
            .asks_on_use("fake-cloud", "not a kind")
            .await
            .map(drop),
    ] {
        assert_eq!(error_name(bad.expect_err("refused")), INVALID_ARGS);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_app_or_a_porter_daemon_asks_about_grants_and_a_stranger_is_denied() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let rig = rig(&dav).await;
    for role in [CallerRole::App, CallerRole::PorterDaemon] {
        let connection = rig.client_as(caller("org.example.Asker", role)).await;
        assert!(
            grants_of(&connection).await.grants().await.is_ok(),
            "{role:?}"
        );
    }
    for role in [
        CallerRole::Settings,
        CallerRole::SheetHost,
        CallerRole::Agent,
        CallerRole::Cua,
        CallerRole::AgentLauncher,
        CallerRole::AiBroker,
        CallerRole::Terminal,
        CallerRole::Compositor,
    ] {
        let connection = rig.client_as(caller("org.example.Refused", role)).await;
        let refused = grants_of(&connection)
            .await
            .grants()
            .await
            .expect_err("refused");
        assert_eq!(error_name(refused), ACCESS_DENIED, "{role:?}");
    }
    let stranger = rig.stranger().await;
    let refused = grants_of(&stranger)
        .await
        .grants()
        .await
        .expect_err("a stranger");
    assert_eq!(error_name(refused), ACCESS_DENIED);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_accounts_are_listed_to_a_porter_daemon_settings_and_the_shell_and_to_no_app() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let rig = rig(&dav).await;
    let account = dav_account(&dav);
    for role in [
        CallerRole::PorterDaemon,
        CallerRole::Settings,
        CallerRole::SheetHost,
    ] {
        let connection = rig.client_as(caller("org.example.Lister", role)).await;
        let text = grants_of(&connection)
            .await
            .accounts()
            .await
            .expect("accounts");
        let listed: Vec<(Account, String)> = from_text(&text).expect("rows");
        assert_eq!(listed.len(), 1, "{role:?}");
        assert_eq!(listed[0].0, account, "{role:?}");
        // No provider file is loaded in the rig, so accountd has no name for the provider.
        assert_eq!(listed[0].1, "", "{role:?}");
        assert!(!text.contains(APP_PASSWORD), "{text}");
    }
    let app = rig.client("org.quire.Photos").await;
    let refused = grants_of(&app).await.accounts().await.expect_err("an app");
    assert_eq!(error_name(refused), ACCESS_DENIED);
}

#[tokio::test(flavor = "multi_thread")]
async fn extending_access_names_an_account_it_holds_and_a_provider_that_asks_on_use() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let rig = rig(&dav).await;
    let daemon = rig
        .client_as(caller("org.quire.Sync", CallerRole::PorterDaemon))
        .await;
    let proxy = grants_of(&daemon).await;
    let nobody = AccountId::parse("nobody").expect("id");
    let refused = proxy
        .extend_access(nobody.as_str(), &["storage"], "")
        .await
        .expect_err("no such account");
    assert_eq!(error_name(refused), UNKNOWN_GRANT);
    let bad = proxy
        .extend_access(dav_account(&dav).id.as_str(), &["not a kind"], "")
        .await
        .expect_err("a kind that is none");
    assert_eq!(error_name(bad), INVALID_ARGS);
    // A provider accountd has no file for asks nothing on use.
    let provider = ProviderId::parse("no-such-provider").expect("id");
    assert!(
        !proxy
            .asks_on_use(provider.as_str(), "storage")
            .await
            .expect("answered")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_asked_for_one_account_gives_that_account_or_a_refusal_never_another() {
    let dav = FakeDav::start("alice", APP_PASSWORD).await.expect("dav");
    let (first, second) = (dav_account(&dav), second_account(&dav));

    // The person allows the first account whatever the sheet is about; the app asked about the
    // second. The answer is not the first account's grant, and nothing is kept for it.
    let rig = rig_of(vec![first.clone(), second.clone()], first.id.clone()).await;
    let connection = rig.client("org.quire.Photos").await;
    let proxy = grants_of(&connection).await;
    let reply = request(&connection, &proxy, second.id.as_str())
        .await
        .expect("asked");
    assert!(matches!(reply, AccountsReply::Refused(_)), "{reply:?}");
    let held: Vec<Grant> = from_text(&proxy.grants().await.expect("grants")).expect("grants");
    assert!(held.is_empty(), "{held:?}");

    // The person allows the second: that is the account granted, though the first fits too.
    let rig = rig_of(vec![first.clone(), second.clone()], second.id.clone()).await;
    let connection = rig.client("org.quire.Photos").await;
    let proxy = grants_of(&connection).await;
    let AccountsReply::Chosen(chosen) = request(&connection, &proxy, second.id.as_str())
        .await
        .expect("asked")
    else {
        panic!("the second account was allowed");
    };
    assert_eq!(chosen.account, second.id);
    let held: Vec<Grant> = from_text(&proxy.grants().await.expect("grants")).expect("grants");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].key.account, second.id);

    // Without a name the person still picks among the accounts that fit.
    let rig = rig_of(vec![first.clone(), second], first.id.clone()).await;
    let connection = rig.client("org.quire.Photos").await;
    let proxy = grants_of(&connection).await;
    let AccountsReply::Chosen(chosen) = request(&connection, &proxy, "").await.expect("asked")
    else {
        panic!("the first account was allowed");
    };
    assert_eq!(chosen.account, first.id);

    // A name that is no account id is an invalid argument.
    let bad = request(&connection, &proxy, "not an id").await;
    assert_eq!(error_name(bad.expect_err("invalid")), INVALID_ARGS);
}

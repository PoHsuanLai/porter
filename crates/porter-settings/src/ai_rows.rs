//! The rows only an AI account or an assistant has: a switch for each service, a group of rows
//! for each permit, and for an assistant that is signed in a way to sign out.

use crate::accounts::since_text;
use crate::helpers::{button, off, readout, refused, spec};
use crate::{AccountRow, Acts, At, Boxed, toggle_of};
use ds_settings::live::LiveError;
use ds_settings::schema::{ActionWeight, Exposure, KeyKind, KeySpec};
use porter_capability::{AbsentReason, CapabilityKind, Offer};
use porter_core::consent::{Decision, Grant, GrantScope};
use porter_core::{Account, AccountState, AuthKind, GrantId};
use porter_ids::word::{from_word, word};

/// The kinds of service of `account` that have a switch: the AI and assistant services it
/// offers, and the ones the person turned off. An account with several models holds one claim
/// per model of the same kind, and has one switch.
fn switch_kinds(account: &Account) -> Vec<CapabilityKind> {
    let mut kinds: Vec<CapabilityKind> = Vec::new();
    for kind in account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(_) => Some(claim.offer.kind()),
            Offer::Absent {
                kind,
                reason: AbsentReason::TurnedOff,
            } => Some(*kind),
            Offer::Absent { .. } => None,
        })
        .filter(|kind| kind.is_ai() || *kind == CapabilityKind::Agent)
    {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// `accounts.<id>.service.<kind>`: the switch for one service of an AI account or an assistant.
#[derive(Debug)]
pub(crate) struct Service;

impl Service {
    /// The kind a path after `service.` names.
    fn kind(tail: &str) -> Option<CapabilityKind> {
        from_word(tail.strip_prefix("service.")?)
    }
}

impl AccountRow for Service {
    fn suffix(&self) -> &'static str {
        "service"
    }

    fn claims(&self, tail: &str, account: &Account, _: &[Grant]) -> bool {
        Self::kind(tail).is_some_and(|kind| switch_kinds(account).contains(&kind))
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        switch_kinds(at.account)
            .into_iter()
            .map(|kind| {
                spec(
                    at.path(&format!("service.{}", word(&kind))),
                    at.section(),
                    kind.name().to_owned(),
                    "Whether apps may use this service of the account.",
                    KeyKind::Toggle {
                        variants: ["on".to_owned(), "off".to_owned()],
                    },
                    toml::Value::String("on".to_owned()),
                    Exposure::Basic,
                )
            })
            .collect()
    }

    fn read(&self, tail: &str, at: &At<'_>) -> Option<toml::Value> {
        let kind = Self::kind(tail)?;
        let on = at
            .account
            .capabilities
            .iter()
            .any(|c| c.offer.kind() == kind && matches!(c.offer, Offer::Present(_)));
        Some(toml::Value::String(
            if on { "on" } else { "off" }.to_owned(),
        ))
    }

    fn write<'a>(
        &'a self,
        tail: &'a str,
        value: toml::Value,
        at: &'a At<'a>,
        acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        let Some(kind) = Self::kind(tail) else {
            return refused(LiveError::UnknownKey(tail.to_owned()));
        };
        match toggle_of(&value) {
            Some(to) => acts.switch(&at.account.id, kind, to),
            None => refused(LiveError::BadValue("a service is `on` or `off`".into())),
        }
    }
}

/// `accounts.<id>.sign_out`: for an assistant that signs itself in and is signed in, asks it to
/// sign itself out.
#[derive(Debug)]
pub(crate) struct SignOut;

impl AccountRow for SignOut {
    fn suffix(&self) -> &'static str {
        "sign_out"
    }

    fn claims(&self, tail: &str, account: &Account, _: &[Grant]) -> bool {
        tail == "sign_out" && account.auth == AuthKind::AgentLogin
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        // An assistant that is signed in signs itself out from here; one that is signed out signs in.
        if at.account.auth != AuthKind::AgentLogin || at.account.state == AccountState::NeedsLogin {
            return Vec::new();
        }
        vec![button(
            at.path("sign_out"),
            at.section(),
            "Sign out",
            "Asks the assistant to sign itself out when it is running; otherwise it only forgets here that it was signed in, and the assistant's own login is not touched.",
            "Sign out",
            ActionWeight::Plain,
        )]
    }

    fn read(&self, _: &str, _: &At<'_>) -> Option<toml::Value> {
        Some(off())
    }

    fn write<'a>(
        &'a self,
        _: &'a str,
        _: toml::Value,
        at: &'a At<'a>,
        acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        acts.sign_out(&at.account.id)
    }
}

/// How long a permit holds, as a person reads it.
const SCOPE_WORDS: [(&str, &str); 3] = [
    ("once", "Once"),
    ("always", "Always"),
    ("session", "This session only"),
];

/// A permit's row: "Companion can use Language model" (allowed) or "Mail can't use Language
/// model" (refused), and "... (this session)" for one that lasts one launcher session.
fn grant_label(grant: &Grant, at: &At<'_>) -> String {
    let can = match grant.decision {
        Decision::Allow => "can",
        Decision::Deny => "can't",
    };
    let lasting = match grant.scope {
        GrantScope::Session(_) => " (this session)",
        GrantScope::Once | GrantScope::Always => "",
    };
    format!(
        "{} {can} use {}{lasting}",
        at.directory.app_title(&grant.key.app.name),
        grant.key.kind.name()
    )
}

/// One of the rows of a permit: `accounts.<id>.grant.<grant>` and its ending.
pub trait GrantRow: Sync + std::fmt::Debug {
    /// What follows the permit's id in the row's path: nothing for the permit's own row,
    /// `.since` and the like for the rest.
    fn ending(&self) -> &'static str;

    /// The key that lists the row for `grant` at `path`.
    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec;

    /// The value a read of the row answers.
    fn read(&self, grant: &Grant, at: &At<'_>) -> toml::Value;

    /// What a write to the row does. A read-out is not written.
    fn write<'a>(
        &'a self,
        _grant: &'a Grant,
        _acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        refused(LiveError::NotPermitted("this row is a read-out".into()))
    }
}

/// The permit's own row: Revoke.
#[derive(Debug)]
struct Revoke;

impl GrantRow for Revoke {
    fn ending(&self) -> &'static str {
        ""
    }

    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec {
        button(
            path,
            at.section(),
            &grant_label(grant, at),
            "Revoking asks the app again the next time it needs the account.",
            "Revoke",
            ActionWeight::Plain,
        )
    }

    fn read(&self, _: &Grant, _: &At<'_>) -> toml::Value {
        off()
    }

    fn write<'a>(
        &'a self,
        grant: &'a Grant,
        acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        acts.revoke(&grant.id)
    }
}

/// `.since`: the UTC day the permit was given.
#[derive(Debug)]
struct Since;

impl GrantRow for Since {
    fn ending(&self) -> &'static str {
        ".since"
    }

    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec {
        readout(path, at.section(), "Since", &since_text(grant), &[])
    }

    fn read(&self, grant: &Grant, _: &At<'_>) -> toml::Value {
        toml::Value::String(since_text(grant))
    }
}

/// `.scope`: how long the permit holds.
#[derive(Debug)]
struct Scope;

impl GrantRow for Scope {
    fn ending(&self) -> &'static str {
        ".scope"
    }

    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec {
        readout(
            path,
            at.section(),
            "Lasts",
            grant.scope.word(),
            &SCOPE_WORDS,
        )
    }

    fn read(&self, grant: &Grant, _: &At<'_>) -> toml::Value {
        toml::Value::String(grant.scope.word().to_owned())
    }
}

/// `.app`: the app the permit is for.
#[derive(Debug)]
struct App;

impl GrantRow for App {
    fn ending(&self) -> &'static str {
        ".app"
    }

    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec {
        let app = grant.key.app.name.as_str();
        let title = at.directory.app_title(&grant.key.app.name);
        readout(path, at.section(), "App", app, &[(app, &title)])
    }

    fn read(&self, grant: &Grant, _: &At<'_>) -> toml::Value {
        toml::Value::String(grant.key.app.name.as_str().to_owned())
    }
}

/// `.service`: the service the permit reaches.
#[derive(Debug)]
struct GrantService;

impl GrantRow for GrantService {
    fn ending(&self) -> &'static str {
        ".service"
    }

    fn spec(&self, path: String, grant: &Grant, at: &At<'_>) -> KeySpec {
        let service = word(&grant.key.kind);
        readout(
            path,
            at.section(),
            "Service",
            service,
            &[(service, grant.key.kind.name())],
        )
    }

    fn read(&self, grant: &Grant, _: &At<'_>) -> toml::Value {
        toml::Value::String(word(&grant.key.kind).to_owned())
    }
}

/// `accounts.<id>.grant.<grant>`: the permit's Revoke.
pub const REVOKE: &dyn GrantRow = &Revoke;
/// `accounts.<id>.grant.<grant>.since`.
pub const SINCE: &dyn GrantRow = &Since;
/// `accounts.<id>.grant.<grant>.scope`.
pub const SCOPE: &dyn GrantRow = &Scope;
/// `accounts.<id>.grant.<grant>.app`.
pub const APP: &dyn GrantRow = &App;
/// `accounts.<id>.grant.<grant>.service`.
pub const GRANT_SERVICE: &dyn GrantRow = &GrantService;

/// The rows of a permit, in the order they are listed.
const GRANT_ROWS: [&dyn GrantRow; 5] = [REVOKE, SINCE, SCOPE, APP, GRANT_SERVICE];

/// The rows of every permit of the account: one [`GrantRow`] each, found by the ending of the
/// path after the permit's id (an id may hold dots, so an ending counts only when what is left
/// names a permit the registry has).
#[derive(Debug)]
pub(crate) struct Permits;

impl Permits {
    /// The row and the permit a path after `accounts.<id>.` names.
    fn find<'a>(tail: &str, grants: &'a [Grant]) -> Option<(&'static dyn GrantRow, &'a Grant)> {
        let rest = tail.strip_prefix("grant.")?;
        // The longer endings first: `.since` is the end of an id as well as of a row.
        GRANT_ROWS.iter().rev().find_map(|row| {
            let id = GrantId::parse(rest.strip_suffix(row.ending())?).ok()?;
            let grant = grants.iter().find(|held| held.id == id)?;
            Some((*row, grant))
        })
    }
}

impl AccountRow for Permits {
    fn suffix(&self) -> &'static str {
        "grant"
    }

    fn claims(&self, tail: &str, _: &Account, grants: &[Grant]) -> bool {
        Self::find(tail, grants).is_some()
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        at.grants
            .iter()
            .filter(|grant| grant.key.account == at.account.id)
            .flat_map(|grant| {
                GRANT_ROWS.iter().map(move |row| {
                    let path = at.path(&format!("grant.{}{}", grant.id, row.ending()));
                    row.spec(path, grant, at)
                })
            })
            .collect()
    }

    fn read(&self, tail: &str, at: &At<'_>) -> Option<toml::Value> {
        let (row, grant) = Self::find(tail, at.grants)?;
        Some(row.read(grant, at))
    }

    fn write<'a>(
        &'a self,
        tail: &'a str,
        _: toml::Value,
        at: &'a At<'a>,
        acts: &'a dyn Acts,
    ) -> Boxed<'a, Result<(), LiveError>> {
        match Self::find(tail, at.grants) {
            Some((row, grant)) => row.write(grant, acts),
            None => refused(LiveError::UnknownKey(tail.to_owned())),
        }
    }
}

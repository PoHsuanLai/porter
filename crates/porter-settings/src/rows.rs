//! The rows every account has.

use crate::helpers::{action, off, readout, spec};
use crate::{AccountRow, Names, Row};
use ds_settings::schema::{ActionWeight, Exposure, KeySpec};
use porter_core::{Account, AccountState, AuthKind};
use porter_ids::word::word as word_of;
use porter_sheet::ProviderGroup;
use porter_sheet::words::group_heading;

/// Where an account lives, as the `place` row says it: a program on this computer (a probed
/// runtime, which signs in with nothing) or anywhere else. Settings lists the first kind under
/// "On this computer".
pub fn place_slug(account: &Account) -> &'static str {
    match account.auth {
        AuthKind::LocalRuntime => "this_computer",
        _ => "elsewhere",
    }
}

fn text(word: &str) -> toml::Value {
    toml::Value::String(word.to_owned())
}

/// A row that shows `word` and nothing else.
fn fixed(account: &Account, suffix: &str, section: &str, label: &str, word: &str) -> KeySpec {
    readout(
        format!("accounts.{}.{suffix}", account.id),
        section,
        label,
        word,
        &[],
    )
}

/// `accounts.<id>.label`: the account's own name.
#[derive(Debug)]
struct Label;

impl AccountRow for Label {
    fn suffix(&self) -> &'static str {
        "label"
    }

    fn spec(&self, account: &Account, section: &str, _: &Names<'_>) -> Option<KeySpec> {
        Some(fixed(
            account,
            "label",
            section,
            "Account",
            &account.label.0,
        ))
    }

    fn read(&self, account: &Account, _: &Names<'_>) -> toml::Value {
        text(&account.label.0)
    }
}

/// `accounts.<id>.place`.
#[derive(Debug)]
struct Place;

impl AccountRow for Place {
    fn suffix(&self) -> &'static str {
        "place"
    }

    fn spec(&self, account: &Account, section: &str, _: &Names<'_>) -> Option<KeySpec> {
        Some(fixed(
            account,
            "place",
            section,
            "Where",
            place_slug(account),
        ))
    }

    fn read(&self, account: &Account, _: &Names<'_>) -> toml::Value {
        text(place_slug(account))
    }
}

/// `accounts.<id>.state`.
#[derive(Debug)]
struct State;

impl AccountRow for State {
    fn suffix(&self) -> &'static str {
        "state"
    }

    fn spec(&self, account: &Account, section: &str, _: &Names<'_>) -> Option<KeySpec> {
        Some(fixed(
            account,
            "state",
            section,
            "Status",
            word_of(&account.state),
        ))
    }

    fn read(&self, account: &Account, _: &Names<'_>) -> toml::Value {
        text(word_of(&account.state))
    }
}

/// `accounts.<id>.sign_in`: how the account is signed in again (`SignInWay`'s slug: `browser`,
/// `password`, `key`, `agent`, `outside`, `nothing`).
#[derive(Debug)]
struct SignIn;

impl AccountRow for SignIn {
    fn suffix(&self) -> &'static str {
        "sign_in"
    }

    fn spec(&self, account: &Account, section: &str, _: &Names<'_>) -> Option<KeySpec> {
        Some(fixed(
            account,
            "sign_in",
            section,
            "Signs in",
            word_of(&account.auth.facts().way),
        ))
    }

    fn read(&self, account: &Account, _: &Names<'_>) -> toml::Value {
        text(word_of(&account.auth.facts().way))
    }
}

/// `accounts.<id>.provider`: the provider the account was made from (the value is the
/// provider's id, the row's labels give its name).
#[derive(Debug)]
struct Provider;

impl AccountRow for Provider {
    fn suffix(&self) -> &'static str {
        "provider"
    }

    fn spec(&self, account: &Account, section: &str, names: &Names<'_>) -> Option<KeySpec> {
        Some(readout(
            format!("accounts.{}.provider", account.id),
            section,
            "Account type",
            account.provider.as_str(),
            &[(account.provider.as_str(), names.provider_label)],
        ))
    }

    fn read(&self, account: &Account, _: &Names<'_>) -> toml::Value {
        text(account.provider.as_str())
    }
}

/// `accounts.<id>.group`: the part of the Accounts page the account is listed under
/// (`internet`, `intelligence`, `agent`).
#[derive(Debug)]
struct Group;

impl AccountRow for Group {
    fn suffix(&self) -> &'static str {
        "group"
    }

    fn spec(&self, account: &Account, section: &str, names: &Names<'_>) -> Option<KeySpec> {
        let headings: Vec<(&str, &str)> = ProviderGroup::ALL
            .iter()
            .map(|group| (word_of(group), group_heading(*group)))
            .collect();
        Some(readout(
            format!("accounts.{}.group", account.id),
            section,
            "Listed under",
            word_of(&names.group),
            &headings,
        ))
    }

    fn read(&self, _: &Account, names: &Names<'_>) -> toml::Value {
        text(word_of(&names.group))
    }
}

/// `accounts.<id>.reauth`: "Sign in again", or "Sign in" for a program that holds its own
/// sign-in and says it is signed out. Not offered where there is nothing to sign in to.
#[derive(Debug)]
struct Reauth;

impl AccountRow for Reauth {
    fn suffix(&self) -> &'static str {
        "reauth"
    }

    fn spec(&self, account: &Account, section: &str, _: &Names<'_>) -> Option<KeySpec> {
        let path = format!("accounts.{}.reauth", account.id);
        let (label, help) = match account.auth {
            // Tailscale signed out: "Sign in" hands the person Tailscale's own page; porter takes
            // no password or key of Tailscale's.
            AuthKind::OwnProgram if account.state == AccountState::NeedsLogin => (
                "Sign in",
                "Opens Tailscale's own sign-in page. Your Tailscale password is never seen here.",
            ),
            AuthKind::AgentLogin if account.state == AccountState::NeedsLogin => (
                "Sign in",
                "Asks the assistant to sign itself in, which needs its app to be running.",
            ),
            // Tailscale keeps its own sign-in, and an agent signs out from its own row.
            AuthKind::OwnProgram | AuthKind::AgentLogin => return None,
            // A program on this computer signs in with nothing: there is nothing to sign in to.
            AuthKind::LocalRuntime | AuthKind::None => return None,
            _ => ("Sign in again", ""),
        };
        Some(spec(
            path,
            section,
            label.to_owned(),
            help,
            action(label, ActionWeight::Plain),
            off(),
            Exposure::Basic,
        ))
    }

    fn read(&self, _: &Account, _: &Names<'_>) -> toml::Value {
        off()
    }
}

/// `accounts.<id>.remove`.
#[derive(Debug)]
struct Remove;

impl AccountRow for Remove {
    fn suffix(&self) -> &'static str {
        "remove"
    }

    fn spec(&self, account: &Account, section: &str, names: &Names<'_>) -> Option<KeySpec> {
        let help = match (account.auth, names.group) {
            // Nothing of Tailscale's is stored here, and nothing of Tailscale's is touched.
            (AuthKind::OwnProgram, _) => {
                "Takes this account out of this computer's accounts. It does not sign Tailscale out."
            }
            (_, ProviderGroup::Internet) => {
                "Deletes the account's stored sign-in from this computer."
            }
            _ => "Deletes the account's stored key and every permission given for it.",
        };
        Some(spec(
            format!("accounts.{}.remove", account.id),
            section,
            "Remove account".to_owned(),
            help,
            action("Remove account", ActionWeight::Destructive),
            off(),
            Exposure::Basic,
        ))
    }

    fn read(&self, _: &Account, _: &Names<'_>) -> toml::Value {
        off()
    }
}

/// `accounts.<id>.label`.
pub const LABEL: Row = Row(&Label);
/// `accounts.<id>.place`.
pub const PLACE: Row = Row(&Place);
/// `accounts.<id>.state`.
pub const STATE: Row = Row(&State);
/// `accounts.<id>.sign_in`.
pub const SIGN_IN: Row = Row(&SignIn);
/// `accounts.<id>.provider`.
pub const PROVIDER: Row = Row(&Provider);
/// `accounts.<id>.group`.
pub const GROUP: Row = Row(&Group);
/// `accounts.<id>.reauth`.
pub const REAUTH: Row = Row(&Reauth);
/// `accounts.<id>.remove`.
pub const REMOVE: Row = Row(&Remove);

/// Every row of an account, in the order they are listed: the read-outs, then the actions.
pub const ROWS: &[Row] = &[
    LABEL, PLACE, STATE, SIGN_IN, PROVIDER, GROUP, REAUTH, REMOVE,
];

/// The read-outs every account has, in the order they are listed.
pub const READOUTS: &[Row] = &[LABEL, PLACE, STATE, SIGN_IN, PROVIDER, GROUP];

/// The actions every account may have, in the order they are listed.
pub const ACTIONS: &[Row] = &[REAUTH, REMOVE];

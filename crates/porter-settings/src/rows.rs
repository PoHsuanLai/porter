//! The rows every account has, and the table of every row an account may have.

use crate::ai_rows::{Permits, Service, SignOut};
use crate::helpers::{button, off, readout, refused};
use crate::identity_rows::{Expires, UsedBy};
use crate::{AccountRow, Acts, At, Boxed, Row};
use ds_settings::live::LiveError;
use ds_settings::schema::{ActionWeight, KeySpec};
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

/// A read-out that says one word: `accounts.<id>.<suffix>`. `labels` gives how a person reads each
/// word it may say, for a row whose words are not plain.
struct ReadOut {
    suffix: &'static str,
    label: &'static str,
    word: fn(&At<'_>) -> String,
    labels: fn(&At<'_>) -> Vec<(String, String)>,
}

impl std::fmt::Debug for ReadOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ReadOut").field(&self.suffix).finish()
    }
}

impl AccountRow for ReadOut {
    fn suffix(&self) -> &'static str {
        self.suffix
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        let labels = (self.labels)(at);
        let labels: Vec<(&str, &str)> = labels
            .iter()
            .map(|(w, l)| (w.as_str(), l.as_str()))
            .collect();
        vec![readout(
            at.path(self.suffix),
            at.section(),
            self.label,
            &(self.word)(at),
            &labels,
        )]
    }

    fn read(&self, _: &str, at: &At<'_>) -> Option<toml::Value> {
        Some(toml::Value::String((self.word)(at)))
    }
}

/// No word of the row is labelled.
fn plain(_: &At<'_>) -> Vec<(String, String)> {
    Vec::new()
}

/// `accounts.<id>.label`: the account's own name.
const LABEL_ROW: ReadOut = ReadOut {
    suffix: "label",
    label: "Account",
    word: |at| at.account.label.0.clone(),
    labels: plain,
};

/// `accounts.<id>.place`.
const PLACE_ROW: ReadOut = ReadOut {
    suffix: "place",
    label: "Where",
    word: |at| place_slug(at.account).to_owned(),
    labels: plain,
};

/// `accounts.<id>.state`.
const STATE_ROW: ReadOut = ReadOut {
    suffix: "state",
    label: "Status",
    word: |at| word_of(&at.account.state).to_owned(),
    labels: plain,
};

/// `accounts.<id>.sign_in`: how the account is signed in again (`SignInWay`'s slug: `browser`,
/// `password`, `key`, `agent`, `outside`, `nothing`).
const SIGN_IN_ROW: ReadOut = ReadOut {
    suffix: "sign_in",
    label: "Signs in",
    word: |at| word_of(&at.account.auth.facts().way).to_owned(),
    labels: plain,
};

/// `accounts.<id>.provider`: the provider the account was made from (the value is the
/// provider's id, the row's labels give its name).
const PROVIDER_ROW: ReadOut = ReadOut {
    suffix: "provider",
    label: "Account type",
    word: |at| at.account.provider.as_str().to_owned(),
    labels: |at| {
        let id = at.account.provider.as_str();
        vec![(id.to_owned(), at.provider_label().to_owned())]
    },
};

/// `accounts.<id>.group`: the part of the Accounts page the account is listed under
/// (`internet`, `intelligence`, `agent`).
const GROUP_ROW: ReadOut = ReadOut {
    suffix: "group",
    label: "Listed under",
    word: |at| word_of(&at.group()).to_owned(),
    labels: |_| {
        ProviderGroup::ALL
            .iter()
            .map(|group| (word_of(group).to_owned(), group_heading(*group).to_owned()))
            .collect()
    },
};

/// `accounts.<id>.reauth`: "Sign in again", or "Sign in" for a program that holds its own
/// sign-in and says it is signed out. Not offered where there is nothing to sign in to.
#[derive(Debug)]
struct Reauth;

impl AccountRow for Reauth {
    fn suffix(&self) -> &'static str {
        "reauth"
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        let account = at.account;
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
            AuthKind::OwnProgram | AuthKind::AgentLogin => return Vec::new(),
            // A program on this computer signs in with nothing: there is nothing to sign in to.
            AuthKind::LocalRuntime | AuthKind::None => return Vec::new(),
            _ => ("Sign in again", ""),
        };
        vec![button(
            at.path("reauth"),
            at.section(),
            label,
            help,
            label,
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
        // A program on this computer has nothing to sign in to (the row is not offered).
        match at.account.auth {
            AuthKind::LocalRuntime | AuthKind::None => refused(LiveError::NotPermitted(
                "this account has nothing to sign in to".into(),
            )),
            _ => acts.sign_in_again(at.account),
        }
    }
}

/// `accounts.<id>.remove`.
#[derive(Debug)]
struct Remove;

impl AccountRow for Remove {
    fn suffix(&self) -> &'static str {
        "remove"
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        let account = at.account;
        let help = match (account.auth, at.group()) {
            // Nothing of Tailscale's is stored here, and nothing of Tailscale's is touched.
            (AuthKind::OwnProgram, _) => {
                "Takes this account out of this computer's accounts. It does not sign Tailscale out."
            }
            (_, ProviderGroup::Internet) => {
                "Deletes the account's stored sign-in from this computer."
            }
            _ => "Deletes the account's stored key and every permission given for it.",
        };
        vec![button(
            at.path("remove"),
            at.section(),
            "Remove account",
            help,
            "Remove account",
            ActionWeight::Destructive,
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
        acts.remove(&at.account.id)
    }
}

/// `accounts.<id>.label`.
pub const LABEL: Row = Row(&LABEL_ROW);
/// `accounts.<id>.place`.
pub const PLACE: Row = Row(&PLACE_ROW);
/// `accounts.<id>.state`.
pub const STATE: Row = Row(&STATE_ROW);
/// `accounts.<id>.sign_in`.
pub const SIGN_IN: Row = Row(&SIGN_IN_ROW);
/// `accounts.<id>.provider`.
pub const PROVIDER: Row = Row(&PROVIDER_ROW);
/// `accounts.<id>.group`.
pub const GROUP: Row = Row(&GROUP_ROW);
/// `accounts.<id>.reauth`.
pub const REAUTH: Row = Row(&Reauth);
/// `accounts.<id>.remove`.
pub const REMOVE: Row = Row(&Remove);

/// `accounts.<id>.expires`.
pub const EXPIRES: Row = Row(&Expires);
/// `accounts.<id>.used_by.<n>`.
pub const USED_BY: Row = Row(&UsedBy);
/// `accounts.<id>.service.<kind>`.
pub const SERVICE: Row = Row(&Service);
/// `accounts.<id>.grant.<grant>` and the rows that go with it.
pub const PERMITS: Row = Row(&Permits);
/// `accounts.<id>.sign_out`.
pub const SIGN_OUT: Row = Row(&SignOut);

/// Every row an account may have, in the order they are listed: the read-outs, what is
/// particular to the kind of account, then the actions.
pub const ROWS: &[Row] = &[
    LABEL, PLACE, STATE, SIGN_IN, PROVIDER, GROUP, EXPIRES, USED_BY, SERVICE, PERMITS, REAUTH,
    SIGN_OUT, REMOVE,
];

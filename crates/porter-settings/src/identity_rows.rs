//! The rows only an identity has: when its sign-in expires, and the apps that use it.

use crate::accounts::expiry_text;
use crate::helpers::spec;
use crate::{AccountRow, At};
use ds_settings::schema::{Exposure, KeyKind, KeySpec};
use porter_core::Account;
use porter_core::consent::Grant;

/// A read-out of one line, in the section of the account.
fn line(at: &At<'_>, tail: &str, label: &str, help: &str, text: String) -> KeySpec {
    spec(
        at.path(tail),
        at.section(),
        label.to_owned(),
        help,
        KeyKind::Fixed {
            variant: text.clone(),
        },
        toml::Value::String(text),
        Exposure::Basic,
    )
}

/// `accounts.<id>.expires`: read-only, only for a sign-in that expires (Google, a client in
/// testing) and whose date is known.
#[derive(Debug)]
pub(crate) struct Expires;

impl AccountRow for Expires {
    fn suffix(&self) -> &'static str {
        "expires"
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        expiry_text(at.account)
            .map(|text| {
                line(
                    at,
                    "expires",
                    "Sign-in",
                    "Google ends the sign-ins of an app that is not verified yet after 7 days; signing in again starts another 7.",
                    text,
                )
            })
            .into_iter()
            .collect()
    }

    fn read(&self, _: &str, at: &At<'_>) -> Option<toml::Value> {
        expiry_text(at.account).map(toml::Value::String)
    }
}

/// `accounts.<id>.used_by.<n>`: read-only, the display name of the n-th app (from 0, in name
/// order) that signed in with this identity.
#[derive(Debug)]
pub(crate) struct UsedBy;

impl UsedBy {
    /// The index a path after `used_by.` names.
    fn index(tail: &str) -> Option<usize> {
        tail.strip_prefix("used_by.")?.parse().ok()
    }
}

impl AccountRow for UsedBy {
    fn suffix(&self) -> &'static str {
        "used_by"
    }

    fn claims(&self, tail: &str, _: &Account, _: &[Grant]) -> bool {
        Self::index(tail).is_some()
    }

    fn specs(&self, at: &At<'_>) -> Vec<KeySpec> {
        at.used_by
            .iter()
            .enumerate()
            .map(|(n, app)| {
                let title = at.directory.app_title(app);
                line(at, &format!("used_by.{n}"), "Used by", "", title)
            })
            .collect()
    }

    fn read(&self, tail: &str, at: &At<'_>) -> Option<toml::Value> {
        let app = at.used_by.get(Self::index(tail)?)?;
        Some(toml::Value::String(at.directory.app_title(app)))
    }
}

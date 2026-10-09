//! What inferd asks accountd for a hosted model: which accounts an app holds a grant on
//! (`Peer.Verdicts`) and, for the one it will call, the account's API key (`Peer.ResolveKey`).
//!
//! The key arrives on a sealed memfd, never as text on the bus. It is read once into a
//! [`SecretText`] (whose `Debug` shows nothing) that the caller drops after building one request.
//! [`Accountd`] is the seam: the bus peer in the daemon, a scripted fake in the unit tests.

use porter_core::capability::LlmFeature;
use porter_core::consent::{GrantScope, Usage, Verdict};
use porter_core::need::LlmNeed;
use porter_core::{
    AccountId, AccountState, AppId, DataClass, GrantId, LauncherSession, Need, SecretText, Tokens,
};
use porter_dbus::{Details, PeerProxy, need_to_dbus};
use rustix::fs::{SealFlags, fcntl_get_seals};
use serde::Serialize;
use std::fmt::Debug;
use std::future::Future;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::pin::Pin;

/// A boxed future, so the seam can be a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the consent store says for an app on one account that could serve a language need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountVerdict {
    /// The account.
    pub account: AccountId,
    /// The provider file the account was made from, when accountd says (`provider` in the
    /// answer's details); otherwise the account id's own stem names it (`models::provider_of`).
    pub provider: Option<String>,
    /// What a person reads for the account (`label` in the answer's details), when accountd says.
    pub label: Option<String>,
    /// What a person reads for its provider (`provider_label`), when accountd knows the file.
    pub provider_label: Option<String>,
    /// Whether it works now (`state`), when accountd says; an account that must be signed in
    /// again is not a place that can serve.
    pub state: Option<AccountState>,
    /// The app's grant on it.
    pub verdict: Verdict,
}

/// Why accountd did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountdFault {
    /// Not on the bus, or the call failed.
    Unreachable,
    /// accountd refused this caller or this grant.
    Refused,
    /// The answer was not what the interface promises (a key not on a sealed file).
    Unreadable,
}

/// accountd, as inferd uses it.
pub trait Accountd: Debug + Send + Sync + 'static {
    /// The grants `app` holds, for data of `class` used as `usage`, on every account that serves a language need.
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>>;

    /// The API key behind `grant`.
    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>>;
}

/// The slug of a closed set's value.
fn slug<T: Serialize>(value: &T) -> String {
    crate::settings::slug_of(value)
}

/// What a language need is, for `Verdicts`: chat, any context.
pub(crate) fn llm_need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(0),
    })
}

fn text_of(details: &Details, name: &str) -> Option<String> {
    String::try_from(details.get(name)?.try_clone().ok()?).ok()
}

/// One row of a `Verdicts` answer; `None` for a row that says `granted` without saying which
/// grant (it grants nothing).
fn verdict_of(row: (String, String, Details)) -> Option<AccountVerdict> {
    let (account, word, details) = row;
    let verdict = match word.as_str() {
        "granted" => {
            let grant = GrantId::parse(&text_of(&details, "grant")?).ok()?;
            // The scope's word, and for a session grant the session beside it.
            let session = match text_of(&details, "session") {
                Some(text) => Some(LauncherSession::parse(&text).ok()?),
                None => None,
            };
            let scope = GrantScope::from_words(&text_of(&details, "scope")?, session.as_ref())?;
            Verdict::Granted { grant, scope }
        }
        "denied" => Verdict::Denied,
        "ask" => Verdict::Ask,
        _ => return None,
    };
    Some(AccountVerdict {
        account: AccountId::parse(&account).ok()?,
        provider: text_of(&details, "provider"),
        label: text_of(&details, "label"),
        provider_label: text_of(&details, "provider_label"),
        state: text_of(&details, "state")
            .and_then(|word| serde_json::from_value(serde_json::Value::String(word)).ok()),
        verdict,
    })
}

/// The key on a sealed descriptor: read from the start, and only when no one can still change it.
fn read_key(fd: OwnedFd) -> Result<SecretText, AccountdFault> {
    let sealed = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK;
    let seals = fcntl_get_seals(&fd).map_err(|_| AccountdFault::Unreadable)?;
    if !seals.contains(sealed) {
        return Err(AccountdFault::Unreadable);
    }
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .map_err(|_| AccountdFault::Unreadable)?;
    Ok(SecretText::new(text))
}

fn fault_of(error: &zbus::Error) -> AccountdFault {
    match error {
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied" =>
        {
            AccountdFault::Refused
        }
        zbus::Error::FDO(fdo) if matches!(**fdo, zbus::fdo::Error::AccessDenied(_)) => {
            AccountdFault::Refused
        }
        _ => AccountdFault::Unreachable,
    }
}

/// accountd over the session bus: `org.quire.Accounts1.Peer`, called by a porter daemon.
#[derive(Debug, Clone)]
pub struct PeerAccountd {
    connection: zbus::Connection,
}

impl PeerAccountd {
    /// Asks accountd over `connection`.
    pub fn new(connection: zbus::Connection) -> Self {
        Self { connection }
    }
}

impl Accountd for PeerAccountd {
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>> {
        Box::pin(async move {
            let peer = PeerProxy::new(&self.connection)
                .await
                .map_err(|e| fault_of(&e))?;
            let app = (app.name.as_str().to_owned(), slug(&app.isolation));
            let rows = peer
                .verdicts(
                    &app,
                    &need_to_dbus(&llm_need()),
                    &slug(&class),
                    &slug(&usage),
                )
                .await
                .map_err(|e| fault_of(&e))?;
            Ok(rows.into_iter().filter_map(verdict_of).collect())
        })
    }

    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move {
            let peer = PeerProxy::new(&self.connection)
                .await
                .map_err(|e| fault_of(&e))?;
            let fd = peer
                .resolve_key(grant.as_str())
                .await
                .map_err(|e| fault_of(&e))?;
            read_key(OwnedFd::from(fd))
        })
    }
}

#[cfg(test)]
mod tests;

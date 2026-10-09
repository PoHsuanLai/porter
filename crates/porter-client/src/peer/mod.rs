//! The daemon-only surface of accountd (`org.quire.Accounts1.Peer`), typed: what inferd asks
//! accountd on behalf of an app, and what it tells accountd of a local runtime. Only a porter
//! daemon (caller role `PorterDaemon`) may call it; accountd refuses every other sender, so an
//! app has no use for this module and uses [`crate::Accounts`] instead.
//!
//! ```ignore
//! let peer = PeerAccounts::over(&connection);
//! for row in peer.verdicts(&app, &chat_need(), DataClass::Prompt, Usage::Interactive).await? {
//!     /* row.verdict says whether the app holds a grant on row.account */
//! }
//! let key = peer.resolve_key(&grant).await?;      // read once, then dropped
//! let mut news = peer.news(&me).await?;           // joins accountd's roster
//! while let Some(change) = news.next().await { /* read the accounts again */ }
//! ```
//!
//! Every bus failure is told by one classification ([`PeerError`], built on
//! `porter_dbus::classify`), so a daemon matches on what to do and never on a bus error name.

mod news;
mod verdict;

pub use news::{AccountChange, AccountNews};
pub use verdict::AccountVerdict;

use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{AccountId, AppId, Claim, DataClass, GrantId, Need, SecretText, Tokens};
use porter_dbus::{
    BusConnection, BusError, BusFailure, Details, PeerProxy, classify, is_invalid_args,
    need_to_dbus, to_vardict,
};
use serde::Serialize;

/// Why accountd did not answer a daemon's call. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PeerError {
    /// accountd is not on the bus. Ask again at the next look.
    #[error("no account service reachable")]
    Unreachable,
    /// accountd refused this caller (it is not a porter daemon) or this grant, with the bus's text.
    /// Asking again does not help.
    #[error("refused by accountd: {0}")]
    Denied(String),
    /// accountd did not understand the call: an argument it does not accept (an unknown
    /// provider, a claim it refuses).
    #[error("not accepted by accountd: {0}")]
    Rejected(String),
    /// accountd answered something that is not what the interface promises.
    #[error("malformed answer: {0}")]
    Malformed(String),
    /// The key did not arrive on a sealed file, or could not be read from it.
    #[error("the key is not on a sealed file")]
    Unreadable,
    /// The bus reported any other failure, with its text.
    #[error("the call failed: {0}")]
    Failed(String),
}

impl From<&BusError> for PeerError {
    /// The one reading of a bus error for a daemon: an argument refused as not the interface's
    /// is `Rejected`, and the rest is [`classify`]'s.
    fn from(error: &BusError) -> Self {
        if is_invalid_args(error) {
            return PeerError::Rejected(error.to_string());
        }
        match classify(error) {
            BusFailure::NoDaemon => PeerError::Unreachable,
            BusFailure::Denied(why) => PeerError::Denied(why),
            BusFailure::Other(why) => PeerError::Failed(why),
        }
    }
}

impl From<BusError> for PeerError {
    fn from(error: BusError) -> Self {
        PeerError::from(&error)
    }
}

/// How a local runtime stands, as `ReportLocal` has it. More may be added: match with a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalState {
    /// The runtime answers.
    Ok,
    /// The runtime stopped: its account stays, and goes offline.
    Offline,
}

impl LocalState {
    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            LocalState::Ok => "ok",
            LocalState::Offline => "offline",
        }
    }
}

/// What a language need is, for `Verdicts`: chat, any context.
pub fn chat_need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(0),
    })
}

/// A claim as the bus carries it: the kind's slug and the claim's fields by name, as
/// `Account.Capabilities` has them.
pub fn claim_to_dbus(claim: &Claim) -> Option<(String, Details)> {
    let kind = serde_json::to_value(claim.offer.kind()).ok()?;
    let fields = match serde_json::to_value(claim).ok()? {
        serde_json::Value::Object(fields) => fields,
        _ => return None,
    };
    Some((kind.as_str()?.to_owned(), to_vardict(&fields)))
}

/// The slug of a closed set's value on the bus (empty for a value that has none).
fn slug<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// accountd's `Peer` interface, called by a porter daemon over its own bus connection.
#[derive(Debug, Clone)]
pub struct PeerAccounts {
    connection: BusConnection,
}

impl PeerAccounts {
    /// Asks accountd over `connection` (a clone of the handle: the connection is shared).
    pub fn over(connection: &BusConnection) -> Self {
        Self {
            connection: connection.clone(),
        }
    }

    async fn proxy(&self) -> Result<PeerProxy<'static>, PeerError> {
        Ok(PeerProxy::new(&self.connection).await?)
    }

    /// What the consent store says for `app` on each account whose offer fits `need`, for data of
    /// `class` used as `usage`. A row that is not well formed is left out (it grants nothing).
    pub async fn verdicts(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Result<Vec<AccountVerdict>, PeerError> {
        let app = (app.name.as_str().to_owned(), slug(&app.isolation));
        let rows = self
            .proxy()
            .await?
            .verdicts(&app, &need_to_dbus(need), &slug(&class), &slug(&usage))
            .await?;
        Ok(rows.into_iter().filter_map(verdict::verdict_of).collect())
    }

    /// The API key behind `grant`. It arrives on a sealed file, never as text on the bus, and is
    /// read once into a [`SecretText`] whose `Debug` shows nothing; drop it after one request.
    pub async fn resolve_key(&self, grant: &GrantId) -> Result<SecretText, PeerError> {
        let fd = self.proxy().await?.resolve_key(grant.as_str()).await?;
        verdict::read_key(std::os::fd::OwnedFd::from(fd))
    }

    /// Tells accountd of a probed local runtime of `provider`, its models as `claims`, in
    /// `state`; the id of the runtime's account.
    pub async fn report_local(
        &self,
        provider: &str,
        claims: &[Claim],
        state: LocalState,
    ) -> Result<AccountId, PeerError> {
        let claims: Vec<_> = claims.iter().filter_map(claim_to_dbus).collect();
        let id = self
            .proxy()
            .await?
            .report_local(provider, claims, state.slug())
            .await?;
        AccountId::parse(&id).map_err(|e| PeerError::Malformed(e.to_string()))
    }

    /// The news of accounts coming, going and changing state, as `app`. See [`AccountNews`].
    pub async fn news(&self, app: &AppId) -> Result<AccountNews, PeerError> {
        AccountNews::join(self.clone(), app.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::fdo;

    #[test]
    fn a_bus_error_says_whether_to_ask_again() {
        let table = [
            (fdo::Error::AccessDenied("not a daemon".into()), "denied"),
            (fdo::Error::AuthFailed("no".into()), "denied"),
            (
                fdo::Error::InvalidArgs("no such provider".into()),
                "rejected",
            ),
            (
                fdo::Error::ServiceUnknown("no accountd".into()),
                "unreachable",
            ),
            (fdo::Error::Failed("busy".into()), "failed"),
        ];
        for (error, want) in table {
            let got = match PeerError::from(&zbus::Error::FDO(Box::new(error))) {
                PeerError::Denied(_) => "denied",
                PeerError::Rejected(_) => "rejected",
                PeerError::Unreachable => "unreachable",
                PeerError::Failed(_) => "failed",
                other => panic!("{other:?}"),
            };
            assert_eq!(got, want);
        }
        assert!(matches!(
            PeerError::from(&zbus::Error::InterfaceNotFound),
            PeerError::Failed(_)
        ));
    }

    #[test]
    fn a_runtime_is_ok_or_offline_on_the_bus() {
        assert_eq!(LocalState::Ok.slug(), "ok");
        assert_eq!(LocalState::Offline.slug(), "offline");
    }
}

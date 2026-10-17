//! The person's AI keys, as an app asks for them (`org.quire.Inference1.Keys` on inferd). An AI
//! key is inferd's: whoever uses a credential holds it, and an app never sees one. What an app
//! gets is the person's yes to use their keys, a permit ([`KeyPermitId`]), after inferd shows the
//! consent sheet; and it can send the person to add a key through the same kind of sheet.
//!
//! ```ignore
//! let keys = AiKeys::connect(&connection).await?;
//! match keys.request_key(DataClass::Documents, Usage::Interactive, &window).await {
//!     Ok(permit) => { /* ask inferd for a session as usual; the permit is what lets it */ }
//!     Err(AiKeysError::Refused(Refusal::Dismissed)) => { /* the person closed the sheet */ }
//!     Err(AiKeysError::Refused(Refusal::NoFittingAccount)) => {
//!         keys.add_account(ProviderHint::Any, &window).await?; // then ask again
//!     }
//!     Err(other) => show(other.to_string()),
//! }
//! ```
//!
//! Settings lists and revokes permits through inferd's settings rows, not here. The agent
//! launcher, the computer-use daemon, the terminal and the compositor are refused.

use crate::abandon::Abandon;
use porter_core::consent::Usage;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{AccountId, AccountsReply, CoreError, DataClass, KeyPermitId};
use porter_dbus::{
    BusConnection, BusError, BusFailure, KeysProxy, ServiceRoot, Sheet, SheetError, SheetKind,
    classify, is_invalid_args, refusal_of, reply_of,
};

/// Why a call to inferd's Keys failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AiKeysError {
    /// inferd refused for a reason the person or the machine gave: the person closed the sheet
    /// (`Dismissed`) or refused (`Denied`), no key fits (`NoFittingAccount`), a key must be signed
    /// in again (`NeedsReauth`), or the service cannot do it now (`Unavailable`).
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// `add_account` for a key the person already has: nothing was added.
    #[error("that account is already added")]
    AlreadyAdded(AccountId),
    /// inferd does not know this caller, or this caller may not ask.
    #[error("refused by inferd: {0}")]
    Denied(String),
    /// inferd refused an argument.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// inferd is not there.
    #[error("no inference service reachable")]
    Unreachable,
    /// inferd sent something that is not porter's protocol.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for AiKeysError {
    fn from(error: BusError) -> Self {
        if let Some(refusal) = refusal_of(&error) {
            return AiKeysError::Refused(refusal);
        }
        if is_invalid_args(&error) {
            return AiKeysError::Invalid(error.to_string());
        }
        match classify(&error) {
            BusFailure::NoDaemon => AiKeysError::Unreachable,
            BusFailure::Denied(why) => AiKeysError::Denied(why),
            BusFailure::Other(why) => AiKeysError::Malformed(why),
        }
    }
}

impl From<CoreError> for AiKeysError {
    fn from(error: CoreError) -> Self {
        AiKeysError::Malformed(error.to_string())
    }
}

impl From<SheetError> for AiKeysError {
    fn from(error: SheetError) -> Self {
        match error {
            SheetError::Bus(error) => error.into(),
            SheetError::Gone => AiKeysError::Unreachable,
            SheetError::Unreadable => {
                AiKeysError::Malformed("a Response that is not (u, a{sv})".into())
            }
        }
    }
}

/// A closed set's serde form is its slug on the bus too.
fn slug<T: serde::Serialize>(value: &T) -> Result<String, AiKeysError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(AiKeysError::Malformed("not a slug".into())),
    }
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Handle(handle) => handle.clone(),
        _ => String::new(),
    }
}

/// inferd's Keys interface, as an app uses it.
#[derive(Debug, Clone)]
pub struct AiKeys {
    connection: BusConnection,
    proxy: KeysProxy<'static>,
}

impl AiKeys {
    /// The Keys interface over `connection`, which inferd knows as an app.
    pub async fn connect(connection: &BusConnection) -> Result<Self, AiKeysError> {
        Self::connect_at(connection, &crate::Destination::WellKnown).await
    }

    /// The same, sent to `to`: the owner of inferd's name the caller checked.
    pub async fn connect_at(
        connection: &BusConnection,
        to: &crate::Destination,
    ) -> Result<Self, AiKeysError> {
        Ok(Self {
            connection: connection.clone(),
            proxy: to.proxy(connection).await?,
        })
    }

    /// Shows the person inferd's consent sheet for this app's use of one of their AI keys, for
    /// data of `class` used as `usage` says, over `window`. The permit it answers is the person's
    /// yes, within the scope they chose.
    pub async fn request_key(
        &self,
        class: DataClass,
        usage: Usage,
        window: &ParentWindow,
    ) -> Result<KeyPermitId, AiKeysError> {
        let (class, usage, window) = (slug(&class)?, slug(&usage)?, wire(window));
        let reply = self
            .sheet(SheetKind::Choose, |options| async move {
                self.proxy
                    .request_key(&class, &usage, &window, &options)
                    .await
            })
            .await?;
        match reply {
            // The answer is the chosen key; its `grant` is the permit's id.
            AccountsReply::Chosen(candidate) => Ok(candidate.grant.into()),
            AccountsReply::Refused(refusal) => Err(AiKeysError::Refused(refusal)),
            other => Err(AiKeysError::Malformed(format!(
                "a key request answered with {other:?}"
            ))),
        }
    }

    /// Shows the sheet that adds an AI key or an agent's account, for `hint` (or the whole list,
    /// [`ProviderHint::Any`]); the account added. A key the person already has is
    /// [`AiKeysError::AlreadyAdded`].
    pub async fn add_account(
        &self,
        hint: ProviderHint,
        window: &ParentWindow,
    ) -> Result<AccountId, AiKeysError> {
        let hint = match hint {
            ProviderHint::Provider(id) => id.as_str().to_owned(),
            _ => String::new(),
        };
        let window = wire(window);
        let reply = self
            .sheet(SheetKind::AddAccount, |options| async move {
                self.proxy.add_account(&hint, &window, &options).await
            })
            .await?;
        match reply {
            AccountsReply::Added(account) => Ok(account),
            AccountsReply::AlreadyAdded(account) => Err(AiKeysError::AlreadyAdded(account)),
            AccountsReply::Refused(refusal) => Err(AiKeysError::Refused(refusal)),
            other => Err(AiKeysError::Malformed(format!(
                "adding an account answered with {other:?}"
            ))),
        }
    }

    /// One sheet method on inferd: listen, call (naming the Request object through
    /// `handle_token`), then wait for the `Response` of the object the call returned. The call
    /// that waits closes the sheet if it is dropped.
    async fn sheet<F, Fut>(&self, kind: SheetKind, call: F) -> Result<AccountsReply, AiKeysError>
    where
        F: FnOnce(porter_dbus::Details) -> Fut,
        Fut: std::future::Future<Output = Result<porter_dbus::zvariant::OwnedObjectPath, BusError>>,
    {
        let mut listener = Sheet::subscribe_on(&self.connection, ServiceRoot::Inference).await?;
        // Armed before the call: a dropped call may already have made the sheet.
        let mut abandon = Abandon(listener.closer(None));
        let path = match call(listener.options()).await {
            Ok(path) => path,
            Err(error) => {
                abandon.0 = None;
                return Err(error.into());
            }
        };
        abandon.0 = listener.closer(Some(path.clone()));
        let (code, results) = listener.response(&path).await?;
        abandon.0 = None;
        Ok(reply_of(kind, code, results)?)
    }
}

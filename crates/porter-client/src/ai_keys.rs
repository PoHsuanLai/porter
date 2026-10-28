//! The person's AI keys, as an app asks for them (`org.quire.Accounts1.Keys` on accountd). An AI
//! key is accountd's, and inferd is the one that sends it to an AI company: an app never sees
//! one. What an app gets is the person's yes to use their keys, a permit ([`GrantId`]), after
//! accountd shows the consent sheet; an app that holds a permit already is answered with it and
//! no sheet is drawn.
//!
//! ```ignore
//! let keys = AiKeys::connect(&connection).await?;
//! match keys.request_key(DataClass::Documents, Usage::Interactive, &window).await {
//!     Ok(permit) => { /* ask inferd for a session as usual; the permit is what lets it */ }
//!     Err(AiKeysError::Refused(Refusal::Dismissed)) => { /* the person closed the sheet */ }
//!     Err(AiKeysError::Refused(Refusal::NoFittingAccount)) => {
//!         // no key is added: send the person to add one (`Accounts::add_account`), then ask again
//!     }
//!     Err(other) => show(other.to_string()),
//! }
//! ```
//!
//! Adding a key is `Accounts::add_account`, the same sheet as for any account. Settings lists
//! and revokes permits through accountd's settings rows, not here. The agent, the agent
//! launcher, the computer-use daemon, the terminal and the compositor are refused.

use crate::abandon::Abandon;
use crate::call_failure::{CallFailure, bus_failure_from};
use porter_core::GrantId;
use porter_core::consent::Usage;
use porter_core::{CoreError, DataClass};
use porter_dbus::accounts::KeysProxy;
use porter_dbus::accounts::refusal_of;
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{ServiceRoot, Sheet, SheetError};
use porter_dbus::{BusConnection, BusError};
use porter_ids::IdError;
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};

/// Why a call to accountd's Keys failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AiKeysError {
    /// accountd refused for a reason the person or the machine gave: the person closed the sheet
    /// (`Dismissed`) or refused (`Denied`), no key fits (`NoFittingAccount`), a key must be signed
    /// in again (`NeedsReauth`), or the service cannot do it now (`Unavailable`).
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// The call itself failed: the service is not there, refused the caller or an argument, or
    /// sent something that is not porter's protocol.
    #[error(transparent)]
    Bus(CallFailure),
}

impl From<BusError> for AiKeysError {
    fn from(error: BusError) -> Self {
        match refusal_of(&error) {
            Some(refusal) => AiKeysError::Refused(refusal),
            None => AiKeysError::Bus(error.into()),
        }
    }
}

bus_failure_from!(AiKeysError: CallFailure, CoreError, IdError);

impl From<SheetError> for AiKeysError {
    fn from(error: SheetError) -> Self {
        match error {
            SheetError::Bus(error) => error.into(),
            SheetError::Gone => CallFailure::Unreachable.into(),
            SheetError::Unreadable => {
                CallFailure::Malformed("a Response that is not (u, a{sv})".into()).into()
            }
            // a failure a later porter-dbus adds
            _ => CallFailure::Malformed("a sheet failure this client does not know".into()).into(),
        }
    }
}

/// A closed set's serde form is its slug on the bus too.
fn slug<T: serde::Serialize>(value: &T) -> Result<String, AiKeysError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(CallFailure::Malformed("not a slug".into()).into()),
    }
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Handle(handle) => handle.clone(),
        _ => String::new(),
    }
}

/// accountd's Keys interface, as an app uses it.
#[derive(Debug, Clone)]
pub struct AiKeys {
    connection: BusConnection,
    proxy: KeysProxy<'static>,
}

impl AiKeys {
    /// The Keys interface over `connection`, which accountd knows as an app.
    pub async fn connect(connection: &BusConnection) -> Result<Self, AiKeysError> {
        Self::connect_at(connection, &porter_dbus::Destination::WellKnown).await
    }

    /// The same, sent to `to`: the owner of accountd's name the caller checked.
    pub async fn connect_at(
        connection: &BusConnection,
        to: &porter_dbus::Destination,
    ) -> Result<Self, AiKeysError> {
        Ok(Self {
            connection: connection.clone(),
            proxy: to.proxy(connection).await?,
        })
    }

    /// Shows the person accountd's consent sheet for this app's use of one of their AI keys, for
    /// data of `class` used as `usage` says, over `window`. The permit it answers is the person's
    /// yes, within the scope they chose.
    pub async fn request_key(
        &self,
        class: DataClass,
        usage: Usage,
        window: &ParentWindow,
    ) -> Result<GrantId, AiKeysError> {
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
            AccountsReply::Chosen(candidate) => Ok(candidate.grant),
            AccountsReply::Refused(refusal) => Err(AiKeysError::Refused(refusal)),
            other => {
                Err(CallFailure::Malformed(format!("a key request answered with {other:?}")).into())
            }
        }
    }

    /// One sheet method on accountd: listen, call (naming the Request object through
    /// `handle_token`), then wait for the `Response` of the object the call returned. The call
    /// that waits closes the sheet if it is dropped.
    async fn sheet<F, Fut>(&self, kind: SheetKind, call: F) -> Result<AccountsReply, AiKeysError>
    where
        F: FnOnce(porter_dbus::Details) -> Fut,
        Fut: std::future::Future<Output = Result<porter_dbus::zvariant::OwnedObjectPath, BusError>>,
    {
        let mut listener = Sheet::subscribe_on(&self.connection, ServiceRoot::Accounts).await?;
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

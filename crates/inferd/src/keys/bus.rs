//! `org.quire.Inference1.Keys` at `/org/quire/Inference1`: how an app gets a permit to use the
//! person's AI keys, and how an AI key or an assistant's account is added through the sheet.
//!
//! Both methods return a Request object (`org.quire.Accounts1.Request`, under
//! `/org/quire/Inference1/request`) and answer on it when the sheet ends, as a portal does:
//!
//! - `RequestKey(class, usage, parent_window, options)`: the consent sheet; the answer carries
//!   the permit's id. The permit is held by the app the caller is, never by a name it sends.
//!   An app that already holds a permit (Always, session or not yet spent) is answered with it
//!   at once and no sheet is drawn ([`KeyHost::answer`]); after a Don't Allow it is refused as
//!   denied, again with no sheet.
//! - `AddAccount(provider_hint, parent_window, options)`: the add sheet for an AI key or an
//!   assistant.
//!
//! Who may call: any app, Settings and the shell. Every other role (an assistant of the
//! desktop, cuad, the terminal, the compositor, the agent launcher, and any role a newer porter
//! adds) is refused: the match fails closed. Settings lists and revokes permits through its
//! settings rows, so there is no method for it here.

use super::host::KeyHost;
use crate::cloud::accountd::llm_need;
use porter_core::ProviderId;
use porter_core::consent::Usage;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{AccountsRequest, DataClass};
use porter_dbus::callers::{CallerRole, Callers};
use porter_dbus::sheet::SheetKind;
use porter_dbus::{Details, ServiceRoot, refusal_error_name_in};
use porter_sheets::RequestObjects;
use std::sync::Arc;
use zbus::Connection;
use zbus::DBusError;
use zbus::fdo;
use zbus::message::{Header, Message};
use zbus::names::ErrorName;
use zbus::zvariant::OwnedObjectPath;

/// An error reply: its name and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysError {
    name: ErrorName<'static>,
    text: String,
}

impl KeysError {
    fn bus(error: fdo::Error) -> Self {
        Self {
            name: error.name().into_owned(),
            text: error.description().unwrap_or_default().to_owned(),
        }
    }

    fn invalid(why: impl ToString) -> Self {
        Self::bus(fdo::Error::InvalidArgs(why.to_string()))
    }

    fn access_denied(why: impl ToString) -> Self {
        Self::bus(fdo::Error::AccessDenied(why.to_string()))
    }

    fn failed(why: impl ToString) -> Self {
        Self::bus(fdo::Error::Failed(why.to_string()))
    }

    /// The error for `refusal`, under `org.quire.Inference1.Error`.
    pub fn of(refusal: Refusal) -> Self {
        let name = ErrorName::try_from(refusal_error_name_in(ServiceRoot::Inference, &refusal))
            .unwrap_or_else(|_| {
                ErrorName::from_static_str_unchecked("org.freedesktop.DBus.Error.Failed")
            });
        Self {
            name,
            text: String::new(),
        }
    }

    /// The error's D-Bus name.
    pub fn error_name(&self) -> &str {
        self.name.as_str()
    }
}

impl DBusError for KeysError {
    fn create_reply(&self, call: &Header<'_>) -> zbus::Result<Message> {
        Message::error(call, self.name.clone())?.build(&(self.text.as_str(),))
    }

    fn name(&self) -> ErrorName<'_> {
        self.name.clone()
    }

    fn description(&self) -> Option<&str> {
        Some(self.text.as_str())
    }
}

/// The `parent_window` argument: empty is no parent.
fn window(text: &str) -> ParentWindow {
    match text.is_empty() {
        true => ParentWindow::Unparented,
        false => ParentWindow::Handle(text.to_owned()),
    }
}

fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, KeysError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| KeysError::invalid(format!("`{text}`: {e}")))
}

/// The Keys object over the hosted service `H`, callers known through `C`.
pub struct KeysObject<H, C> {
    host: Arc<H>,
    callers: Arc<C>,
    objects: RequestObjects,
}

impl<H, C> KeysObject<H, C> {
    /// The object over `host`, served on `connection`; a sender is identified through `callers`.
    pub fn new(host: Arc<H>, callers: Arc<C>, connection: Connection) -> Self {
        Self {
            host,
            callers,
            objects: RequestObjects::new(connection, ServiceRoot::Inference),
        }
    }
}

impl<H, C> std::fmt::Debug for KeysObject<H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeysObject").finish_non_exhaustive()
    }
}

impl<H: KeyHost, C: Callers> KeysObject<H, C> {
    /// The app behind the sender of a call, when its role may ask: an app, Settings or the shell.
    async fn asker(&self, header: &Header<'_>) -> Result<porter_core::AppId, KeysError> {
        let sender = header
            .sender()
            .ok_or_else(|| KeysError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| KeysError::access_denied("inferd does not know this caller"))?;
        match caller.role {
            CallerRole::App | CallerRole::Settings | CallerRole::SheetHost => Ok(caller.app),
            _ => Err(KeysError::access_denied(
                "this caller may not ask for the person's AI keys",
            )),
        }
    }

    /// Starts a `kind` sheet that answers `request` for the app behind the sender of `header`:
    /// one per app and kind, under the Request object the options name. Returns its path.
    async fn sheet(
        &self,
        header: &Header<'_>,
        kind: SheetKind,
        options: &Details,
        request: AccountsRequest,
    ) -> Result<OwnedObjectPath, KeysError> {
        let app = self.asker(header).await?;
        let sender = header
            .sender()
            .ok_or_else(|| KeysError::access_denied("no sender"))?
            .to_owned();
        let claim = self.objects.claim(&app, kind).map_err(KeysError::failed)?;
        let path = self
            .objects
            .path(&sender, options)
            .map_err(KeysError::failed)?;
        let host = Arc::clone(&self.host);
        let run = async move {
            // Held until the sheet ends, however it ends.
            let _claim = claim;
            host.answer(&app, request).await
        };
        self.objects
            .start(sender, path, kind, run)
            .await
            .map_err(KeysError::failed)
    }
}

#[zbus::interface(name = "org.quire.Inference1.Keys")]
impl<H: KeyHost, C: Callers> KeysObject<H, C> {
    async fn request_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, KeysError> {
        let class: DataClass = slug(&class)?;
        let usage: Usage = slug(&usage)?;
        let request = AccountsRequest::Choose {
            need: llm_need(),
            class,
            usage,
            window: window(&parent_window),
        };
        self.sheet(&header, SheetKind::Choose, &options, request)
            .await
    }

    async fn add_account(
        &self,
        #[zbus(header)] header: Header<'_>,
        provider_hint: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, KeysError> {
        let hint = match provider_hint.is_empty() {
            true => ProviderHint::Any,
            false => ProviderId::parse(&provider_hint)
                .map(ProviderHint::Provider)
                .map_err(KeysError::invalid)?,
        };
        let request = AccountsRequest::AddAccount {
            hint,
            window: window(&parent_window),
        };
        self.sheet(&header, SheetKind::AddAccount, &options, request)
            .await
    }
}

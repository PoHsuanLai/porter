//! accountd's calls over the bus: the two sheet methods it still has, `AddAccount` and
//! `Reauthenticate`. accountd keeps identities and nothing grant-keyed, so finding, choosing,
//! tokens, grants and relays are not on the bus; a request for one of them is
//! [`TransportError::Unsupported`], as for any carrier that has no such call. A refusal comes back
//! as the error `org.quire.Accounts1.Error.<Refusal>` and is returned as
//! `AccountsReply::Refused`, as the in-process carrier does.
//!
//! The sheet methods return a Request object whose `Response` signal carries the answer
//! (`porter_dbus::sheet::reply_of` reads it: code 0 the answer, 1 `Dismissed`, 2 the refusal named in
//! the results). The signal may be sent before the method's reply arrives, so the listener is
//! subscribed first (`porter_dbus::Sheet`), and the call that waits closes the sheet if it is
//! dropped.

use super::dbus::bus_error;
use crate::Destination;
use crate::abandon::Abandon;
use crate::error::TransportError;
use porter_core::ProviderHint;
use porter_dbus::names::account_path;
use porter_dbus::sheet::{SheetKind, reply_of};
use porter_dbus::zvariant::OwnedObjectPath;
use porter_dbus::{
    AccountProxy, BusConnection, BusError, Details, ManagerProxy, Sheet, SheetError, refusal_of,
};
use porter_wire::ParentWindow;
use porter_wire::{AccountsReply, AccountsRequest};

/// A failed call: the refusal accountd gave, or a failure of the bus.
fn refused(error: &BusError) -> Result<AccountsReply, TransportError> {
    match refusal_of(error) {
        Some(refusal) => Ok(AccountsReply::Refused(refusal)),
        None => Err(bus_error(error)),
    }
}

fn malformed(error: impl std::fmt::Display) -> TransportError {
    TransportError::Malformed(error.to_string())
}

async fn manager(
    connection: &BusConnection,
    to: &Destination,
) -> Result<ManagerProxy<'static>, TransportError> {
    to.proxy(connection).await.map_err(|e| bus_error(&e))
}

/// One request to accountd over the bus.
pub(super) async fn call(
    connection: &BusConnection,
    to: &Destination,
    request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    match request {
        AccountsRequest::AddAccount { hint, window } => {
            let (hint, window) = (hint_text(&hint), wire(&window));
            let proxy = manager(connection, to).await?;
            sheet(connection, SheetKind::AddAccount, |options| async move {
                proxy.add_account(&hint, &window, &options).await
            })
            .await
        }
        AccountsRequest::Reauthenticate { account, window } => {
            let window = wire(&window);
            let builder = AccountProxy::builder(connection)
                .path(account_path(&account))
                .map_err(|e| bus_error(&e))?;
            let proxy = to
                .aim(builder)
                .map_err(|e| bus_error(&e))?
                .build()
                .await
                .map_err(|e| bus_error(&e))?;
            sheet(
                connection,
                SheetKind::Reauthenticate,
                |options| async move { proxy.reauthenticate(&window, &options).await },
            )
            .await
        }
        // Everything else (a query, a choice, a grant, a token, a relay) is not on the bus.
        _ => Err(TransportError::Unsupported),
    }
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Unparented => String::new(),
        ParentWindow::Handle(handle) => handle.clone(),
        // A parent this build does not know is no parent.
        _ => String::new(),
    }
}

/// The `provider_hint` argument: the provider list is the empty string.
fn hint_text(hint: &ProviderHint) -> String {
    match hint {
        ProviderHint::Any => String::new(),
        ProviderHint::Provider(id) => id.as_str().to_owned(),
    }
}

fn sheet_error(error: SheetError) -> TransportError {
    match error {
        SheetError::Bus(error) => bus_error(&error),
        SheetError::Gone => TransportError::Closed,
        SheetError::Unreadable => {
            TransportError::Malformed("a Response that is not (u, a{sv})".to_owned())
        }
        // a failure a later porter-dbus adds
        _ => TransportError::Malformed("a sheet failure this client does not know".to_owned()),
    }
}

/// One sheet method: listen, call (naming the Request object through `handle_token`), then wait
/// for the `Response` of the object the call returned. An error the call itself returns (a
/// refusal before any sheet) is settled like an immediate call's.
async fn sheet<F, Fut>(
    connection: &BusConnection,
    kind: SheetKind,
    call: F,
) -> Result<AccountsReply, TransportError>
where
    F: FnOnce(Details) -> Fut,
    Fut: std::future::Future<Output = Result<OwnedObjectPath, BusError>>,
{
    let mut listener = Sheet::subscribe(connection)
        .await
        .map_err(|e| bus_error(&e))?;
    // Armed before the call: a dropped call may already have made the object.
    let mut abandon = Abandon(listener.closer(None));
    let path = match call(listener.options()).await {
        Ok(path) => path,
        Err(error) => {
            abandon.answered();
            return refused(&error);
        }
    };
    abandon.0 = listener.closer(Some(path.clone()));
    let (code, results) = listener.response(&path).await.map_err(sheet_error)?;
    abandon.answered();
    reply_of(kind, code, results).map_err(malformed)
}

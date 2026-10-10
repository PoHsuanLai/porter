//! accountd's calls over the bus: the methods that answer at once (`Query`, `Availability`,
//! `Grants.List` and `Revoke`, `Tokens.IssueToken`). A refusal comes back as the error
//! `org.quire.Accounts1.Error.<Refusal>` and is returned as `AccountsReply::Refused`, as the
//! in-process carrier does.
//!
//! The sheet methods (`Choose`, `AddAccount`, `Reauthenticate`) return a Request object whose
//! `Response` signal carries the answer (`porter_dbus::reply_of` reads it: code 0 the answer, 1
//! `Dismissed`, 2 the refusal named in the results). The signal may be sent before the method's
//! reply arrives, so the listener is subscribed first (`porter_dbus::Sheet`), and the call that
//! waits closes the sheet if it is dropped.

use super::dbus::{bus_error, slug};
use crate::error::TransportError;
use porter_core::consent::Availability;
use porter_core::wire::{ParentWindow, ProviderHint};
use porter_core::{AccountsReply, AccountsRequest, Candidate, DataClass, Need};
use porter_dbus::zvariant::OwnedObjectPath;
use porter_dbus::{
    AccountProxy, BusConnection, BusError, Closer, Details, GrantsProxy, ManagerProxy, Sheet,
    SheetError, SheetKind, TokensProxy, account_path, candidate_from_dbus, grant_from_dbus,
    need_to_dbus, refusal_of, reply_of, token_from_dbus,
};

/// A call that came back as a value, or as a refusal accountd gave, or not at all.
fn settled<T>(
    result: Result<T, BusError>,
    reply: impl FnOnce(T) -> Result<AccountsReply, TransportError>,
) -> Result<AccountsReply, TransportError> {
    match result {
        Ok(value) => reply(value),
        Err(error) => refused(&error),
    }
}

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

fn candidates(list: Vec<porter_dbus::CandidateArg>) -> Result<Vec<Candidate>, TransportError> {
    list.into_iter()
        .map(|arg| candidate_from_dbus(arg).map_err(malformed))
        .collect()
}

fn availability(slug: &str) -> Result<Availability, TransportError> {
    serde_json::from_value(serde_json::Value::String(slug.to_owned())).map_err(malformed)
}

async fn manager(connection: &BusConnection) -> Result<ManagerProxy<'_>, TransportError> {
    ManagerProxy::new(connection)
        .await
        .map_err(|e| bus_error(&e))
}

async fn query(
    connection: &BusConnection,
    need: &Need,
    class: DataClass,
    usage: &impl serde::Serialize,
) -> Result<AccountsReply, TransportError> {
    let proxy = manager(connection).await?;
    let result = proxy
        .query(&need_to_dbus(need), &slug(&class)?, &slug(usage)?)
        .await;
    settled(result, |list| {
        candidates(list).map(AccountsReply::Candidates)
    })
}

/// One request to accountd over the bus.
pub(super) async fn call(
    connection: &BusConnection,
    request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    match request {
        AccountsRequest::Query { need, class, usage } => {
            query(connection, &need, class, &usage).await
        }
        AccountsRequest::Availability { need, class, usage } => {
            let proxy = manager(connection).await?;
            let result = proxy
                .availability(&need_to_dbus(&need), &slug(&class)?, &slug(&usage)?)
                .await;
            settled(result, |text| {
                availability(&text).map(AccountsReply::Availability)
            })
        }
        AccountsRequest::ListGrants => {
            let proxy = GrantsProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(proxy.list().await, |list| {
                list.into_iter()
                    .map(|arg| grant_from_dbus(arg).map_err(malformed))
                    .collect::<Result<Vec<_>, _>>()
                    .map(AccountsReply::Grants)
            })
        }
        AccountsRequest::Revoke { grant } => {
            let proxy = GrantsProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(proxy.revoke(grant.as_str()).await, |()| {
                Ok(AccountsReply::Revoked)
            })
        }
        AccountsRequest::IssueToken { grant, audience } => {
            let proxy = TokensProxy::new(connection)
                .await
                .map_err(|e| bus_error(&e))?;
            settled(
                proxy.issue_token(grant.as_str(), &audience.0).await,
                |arg| {
                    token_from_dbus(arg)
                        .map(AccountsReply::Token)
                        .map_err(malformed)
                },
            )
        }
        // The relay's descriptor is out of band, so this request has its own transport method.
        AccountsRequest::OpenAuthenticated { .. } => Err(TransportError::Malformed(
            "OpenAuthenticated carries a descriptor: use `open_authenticated`".to_owned(),
        )),
        AccountsRequest::OpenLinked { .. } => Err(TransportError::Malformed(
            "OpenLinked carries a descriptor: use `open_linked`".to_owned(),
        )),
        AccountsRequest::Choose {
            need,
            class,
            usage,
            window,
        } => {
            let (need, class, usage, window) = (
                need_to_dbus(&need),
                slug(&class)?,
                slug(&usage)?,
                wire(&window),
            );
            let proxy = manager(connection).await?;
            sheet(connection, SheetKind::Choose, |options| async move {
                proxy.choose(&need, &class, &usage, &window, &options).await
            })
            .await
        }
        AccountsRequest::AddAccount { hint, window } => {
            let (hint, window) = (hint_text(&hint), wire(&window));
            let proxy = manager(connection).await?;
            sheet(connection, SheetKind::AddAccount, |options| async move {
                proxy.add_account(&hint, &window, &options).await
            })
            .await
        }
        AccountsRequest::Reauthenticate { account, window } => {
            let window = wire(&window);
            let proxy = AccountProxy::builder(connection)
                .path(account_path(&account))
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
        // a variant a newer porter adds: the bus has no call for it, so it is malformed
        _ => Err(TransportError::Malformed(
            "a request this client has no bus call for".to_owned(),
        )),
    }
}

/// The `parent_window` argument: no parent is the empty string.
fn wire(window: &ParentWindow) -> String {
    match window {
        ParentWindow::Unparented => String::new(),
        ParentWindow::Handle(handle) => handle.clone(),
    }
}

/// The `provider_hint` argument: the provider list is the empty string.
fn hint_text(hint: &ProviderHint) -> String {
    match hint {
        ProviderHint::Any => String::new(),
        ProviderHint::Provider(id) => id.as_str().to_owned(),
    }
}

/// Closes the sheet when the call that waits for it is dropped before it was answered, so an
/// abandoned request does not leave a sheet on the screen. Needs a tokio runtime, as the
/// transport does.
struct Abandon(Option<Closer>);

impl Abandon {
    fn answered(&mut self) {
        self.0 = None;
    }
}

impl Drop for Abandon {
    fn drop(&mut self) {
        let Some(closer) = self.0.take() else { return };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(closer.close());
        }
    }
}

fn sheet_error(error: SheetError) -> TransportError {
    match error {
        SheetError::Bus(error) => bus_error(&error),
        SheetError::Gone => TransportError::Closed,
        SheetError::Unreadable => {
            TransportError::Malformed("a Response that is not (u, a{sv})".to_owned())
        }
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

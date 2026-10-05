//! The Request objects of the sheet methods, shaped as portals' are.
//!
//! A sheet method (`Choose`, `AddAccount`, `Reauthenticate`) registers an
//! `org.quire.Accounts1.Request` object under `porter_dbus::request_path` (the caller's unique
//! name, then its `handle_token` or one minted here), returns its path at once, and runs the
//! service call in a task. When the call ends the task sends `Response(code, results)`
//! (`porter_dbus::response_of` says what the answer looks like) to the caller alone, then removes
//! the object. The signal may therefore reach the caller before the method's reply does; callers
//! subscribe first (`porter_dbus::Sheet`).
//!
//! The sheet ends without a `Response` when the caller calls `Close` or leaves the bus; the call
//! in flight is dropped, which is how the prompter's sheet is taken down. A call that panics
//! answers `Refusal::Unavailable`: the caller is never left waiting on a task that is gone.

use crate::callers::Callers;
use crate::core::{Core, Host, handle_token};
use crate::errors::RefusedError;
use porter_core::wire::Refusal;
use porter_core::{AccountsReply, AccountsRequest};
use porter_dbus::{Details, SheetKind, is_handle_token, request_path, response_of};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::task::{AbortHandle, JoinHandle};
use zbus::export::futures_core::Stream;
use zbus::fdo::{DBusProxy, NameOwnerChangedStream};
use zbus::message::Header;
use zbus::names::{BusName, UniqueName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::{Connection, ObjectServer};

/// The slot the sheet's task puts its abort handle in, for `Close`.
type AbortSlot = Arc<Mutex<Option<AbortHandle>>>;

/// One sheet shown, as an object.
#[derive(Debug)]
pub(crate) struct RequestObject {
    owner: String,
    task: AbortSlot,
}

#[zbus::interface(name = "org.quire.Accounts1.Request")]
impl RequestObject {
    /// Closes the sheet; no `Response` follows. Only the caller that started it may.
    async fn close(&self, #[zbus(header)] header: Header<'_>) -> Result<(), RefusedError> {
        match header.sender().map(UniqueName::as_str) == Some(self.owner.as_str()) {
            true => {
                if let Some(task) = self
                    .task
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    task.abort();
                }
                Ok(())
            }
            false => Err(RefusedError::access_denied(
                "only the caller that opened the sheet may close it",
            )),
        }
    }

    #[zbus(signal)]
    async fn response(
        emitter: &SignalEmitter<'_>,
        response: u32,
        results: Details,
    ) -> zbus::Result<()>;
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Starts a sheet for the caller of `header`: registers its Request object and runs
    /// `request` for the app behind the sender. Returns the object's path.
    pub(crate) async fn sheet(
        self: &Arc<Self>,
        header: &Header<'_>,
        connection: &Connection,
        kind: SheetKind,
        options: &Details,
        request: AccountsRequest,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let app = self.acting(header).await?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        let token = match handle_token(options) {
            Some(token) if is_handle_token(&token) => token,
            Some(token) => {
                return Err(RefusedError::invalid(format!(
                    "handle_token `{token}` is not a path segment"
                )));
            }
            None => format!("accountd_{}", self.minted.fetch_add(1, Ordering::Relaxed)),
        };
        let path = request_path(sender.as_str(), &token)
            .and_then(|path| ObjectPath::try_from(path).ok())
            .map(OwnedObjectPath::from)
            .ok_or_else(|| RefusedError::invalid("no request path for this sender"))?;
        let core = Arc::clone(self);
        let run = async move {
            let reply = core.host.handle(&app, request).await;
            core.publish().await;
            reply
        };
        start(connection, sender, path, kind, run).await
    }
}

/// Registers the object, then runs `run` in a task that answers it.
async fn start(
    connection: &Connection,
    sender: UniqueName<'static>,
    path: OwnedObjectPath,
    kind: SheetKind,
    run: impl Future<Output = AccountsReply> + Send + 'static,
) -> Result<OwnedObjectPath, RefusedError> {
    let leaving = DBusProxy::new(connection)
        .await
        .map_err(RefusedError::failed)?
        .receive_name_owner_changed_with_args(&[(0, sender.as_str())])
        .await
        .map_err(RefusedError::failed)?;
    let slot = AbortSlot::default();
    let object = RequestObject {
        owner: sender.as_str().to_owned(),
        task: Arc::clone(&slot),
    };
    let server = connection.object_server();
    match server.at(&path, object).await {
        Ok(true) => {}
        Ok(false) => return Err(RefusedError::invalid("that handle_token is in use")),
        Err(error) => return Err(RefusedError::failed(error)),
    }
    let inner = tokio::spawn(run);
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(inner.abort_handle());
    tokio::spawn(answer(
        connection.clone(),
        server.clone(),
        sender,
        path.clone(),
        kind,
        inner,
        leaving,
    ));
    Ok(path)
}

/// Waits for the call, sends its `Response` unless the sheet was closed or abandoned, and takes
/// the object away.
async fn answer(
    connection: Connection,
    server: ObjectServer,
    sender: UniqueName<'static>,
    path: OwnedObjectPath,
    kind: SheetKind,
    mut inner: JoinHandle<AccountsReply>,
    leaving: NameOwnerChangedStream,
) {
    let reply = tokio::select! {
        joined = &mut inner => match joined {
            Ok(reply) => Some(reply),
            // Closed by its caller: no answer follows.
            Err(error) if error.is_cancelled() => None,
            Err(_) => Some(AccountsReply::Refused(Refusal::Unavailable)),
        },
        () = departed(leaving) => {
            inner.abort();
            None
        }
    };
    if let Some(reply) = reply {
        let response = response_of(kind, &reply);
        let _ = send(
            &connection,
            &sender,
            &path,
            response.code.to_wire(),
            response.results,
        )
        .await;
    }
    let _ = server.remove::<RequestObject, _>(&path).await;
}

async fn send(
    connection: &Connection,
    sender: &UniqueName<'static>,
    path: &OwnedObjectPath,
    code: u32,
    results: Details,
) -> zbus::Result<()> {
    let emitter = SignalEmitter::new(connection, path.clone())?
        .set_destination(BusName::from(sender.clone()));
    RequestObject::response(&emitter, code, results).await
}

/// Resolves when the caller's connection leaves the bus.
async fn departed(mut owners: NameOwnerChangedStream) {
    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await;
        match next {
            Some(signal) => {
                let gone = signal
                    .args()
                    .map(|args| args.new_owner().is_none())
                    .unwrap_or(false);
                if gone {
                    return;
                }
            }
            None => return,
        }
    }
}

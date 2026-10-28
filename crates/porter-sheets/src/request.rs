//! The Request objects of the sheet methods, shaped as portals' are, for any daemon that shows a
//! sheet.
//!
//! A sheet method registers an `org.quire.Accounts1.Request` object under the request root of its
//! [`ServiceRoot`] (the caller's unique name, then its `handle_token` or one minted here), returns
//! its path at once, and runs the call in a task. When the call ends the task sends
//! `Response(code, results)` (`porter_dbus::sheet::response_of` says what the answer looks like) to the
//! caller alone, then removes the object. The signal may therefore reach the caller before the
//! method's reply does; callers subscribe first (`porter_dbus::Sheet`).
//!
//! The sheet ends without a `Response` when the caller calls `Close` or leaves the bus; the call
//! in flight is dropped, which is how the prompter's sheet is taken down. A call that panics
//! answers `Refusal::Unavailable`: the caller is never left waiting on a task that is gone.
//!
//! Errors are the bus's own (`fdo::Error`): `LimitsExceeded` for an app that has a sheet of that
//! kind open already, `InvalidArgs` for a bad `handle_token`, `AccessDenied` for a `Close` from
//! anyone but the caller.

use porter_core::AppId;
use porter_dbus::sheet::{
    OPTION_HANDLE_TOKEN, SheetKind, is_handle_token, request_path_in, response_of,
};
use porter_dbus::{Call, Details, RequestServed, RequestServer, ServiceRoot};
use porter_wire::AccountsReply;
use porter_wire::Refusal;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::task::{AbortHandle, JoinHandle};
use zbus::export::futures_core::Stream;
use zbus::fdo::{self, DBusProxy, NameOwnerChangedStream};
use zbus::names::{BusName, UniqueName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::{Connection, ObjectServer};

/// The slot the sheet's task puts its abort handle in, for `Close`.
type AbortSlot = Arc<Mutex<Option<AbortHandle>>>;

/// The sheets open now, by the app that asked and the kind of sheet: an app has one of each
/// kind at a time, so it cannot stack sheets on the person. Keyed by the app, not the
/// connection, so a second connection of the same app is the same asker.
#[derive(Debug, Clone, Default)]
pub struct OpenSheets(Arc<Mutex<HashSet<(AppId, SheetKind)>>>);

/// One open sheet, held by its task; dropping it (the sheet answered, was closed, or its caller
/// left) frees the app to ask again.
#[derive(Debug)]
pub struct SheetClaim {
    open: OpenSheets,
    key: (AppId, SheetKind),
}

impl OpenSheets {
    /// A claim on a `kind` sheet for `app`, or `LimitsExceeded` while it has one open: no
    /// `Refusal` says "you have one open already" (each is about the account or the person), so
    /// the method answers the bus's own error and no sheet or Request object is made.
    pub fn claim(&self, app: &AppId, kind: SheetKind) -> Result<SheetClaim, fdo::Error> {
        let key = (app.clone(), kind);
        match self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone())
        {
            true => Ok(SheetClaim {
                open: self.clone(),
                key,
            }),
            false => Err(fdo::Error::LimitsExceeded(
                "this app already has a sheet of this kind open".to_owned(),
            )),
        }
    }
}

impl Drop for SheetClaim {
    fn drop(&mut self) {
        self.open
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// One sheet shown, as an object.
#[derive(Debug)]
struct RequestObject {
    owner: String,
    task: AbortSlot,
}

impl RequestServer for RequestObject {
    type Error = fdo::Error;

    /// Closes the sheet; no `Response` follows. Only the caller that started it may.
    async fn close(&self, Call { header, .. }: Call<'_>) -> fdo::Result<()> {
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
            false => Err(fdo::Error::AccessDenied(
                "only the caller that opened the sheet may close it".to_owned(),
            )),
        }
    }
}

/// The Request objects of one daemon on one connection: makes their paths under the daemon's
/// [`ServiceRoot`], keeps an app to one sheet of a kind, and runs and answers each.
#[derive(Debug, Clone)]
pub struct RequestObjects {
    connection: Connection,
    service: ServiceRoot,
    open: OpenSheets,
    minted: Arc<AtomicU64>,
}

impl RequestObjects {
    /// The Request objects of `service`'s daemon, served on `connection`.
    pub fn new(connection: Connection, service: ServiceRoot) -> Self {
        Self {
            connection,
            service,
            open: OpenSheets::default(),
            minted: Arc::default(),
        }
    }

    /// A claim on a `kind` sheet for `app` ([`OpenSheets::claim`]).
    pub fn claim(&self, app: &AppId, kind: SheetKind) -> Result<SheetClaim, fdo::Error> {
        self.open.claim(app, kind)
    }

    /// The path of the Request object for a sheet `sender` starts: its `handle_token` if the
    /// options name one, else one minted here.
    pub fn path(
        &self,
        sender: &UniqueName<'_>,
        options: &Details,
    ) -> Result<OwnedObjectPath, fdo::Error> {
        let token = match handle_token(options) {
            Some(token) if is_handle_token(&token) => token,
            Some(token) => {
                return Err(fdo::Error::InvalidArgs(format!(
                    "handle_token `{token}` is not a path segment"
                )));
            }
            None => format!(
                "{}_{}",
                self.service_word(),
                self.minted.fetch_add(1, Ordering::Relaxed)
            ),
        };
        request_path_in(self.service, sender.as_str(), &token)
            .and_then(|path| ObjectPath::try_from(path).ok())
            .map(OwnedObjectPath::from)
            .ok_or_else(|| fdo::Error::InvalidArgs("no request path for this sender".to_owned()))
    }

    /// The word minted tokens start with.
    fn service_word(&self) -> &'static str {
        match self.service {
            ServiceRoot::Accounts => "accountd",
            ServiceRoot::Inference => "inferd",
            ServiceRoot::Sync => "syncd",
            _ => "porter",
        }
    }

    /// Registers the object at `path` for the sheet `sender` asked for, then runs `run` in a task
    /// that answers it with a `kind` response. Returns the path.
    pub async fn start(
        &self,
        sender: UniqueName<'static>,
        path: OwnedObjectPath,
        kind: SheetKind,
        run: impl Future<Output = AccountsReply> + Send + 'static,
    ) -> Result<OwnedObjectPath, fdo::Error> {
        let connection = &self.connection;
        let leaving = DBusProxy::new(connection)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?
            .receive_name_owner_changed_with_args(&[(0, sender.as_str())])
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let slot = AbortSlot::default();
        let object = RequestObject {
            owner: sender.as_str().to_owned(),
            task: Arc::clone(&slot),
        };
        let server = connection.object_server();
        match server.at(&path, RequestServed(object)).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(fdo::Error::InvalidArgs(
                    "that handle_token is in use".to_owned(),
                ));
            }
            Err(error) => return Err(fdo::Error::Failed(error.to_string())),
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
}

/// The `handle_token` of a call's options, if it names one.
fn handle_token(options: &Details) -> Option<String> {
    let value = options.get(OPTION_HANDLE_TOKEN)?;
    String::try_from(value.try_clone().ok()?).ok()
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
    let _ = server
        .remove::<RequestServed<RequestObject>, _>(&path)
        .await;
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
    RequestServed::<RequestObject>::response(&emitter, code, results).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AppName, Isolation};

    fn app(name: &str) -> AppId {
        AppId {
            name: AppName::parse(name).expect("app name"),
            isolation: Isolation::Flatpak,
        }
    }

    #[test]
    fn an_app_has_one_open_sheet_of_each_kind_until_it_ends() {
        let open = OpenSheets::default();
        let mail = app("org.quire.Mail");
        let first = open.claim(&mail, SheetKind::AddAccount).expect("first");
        let again = open
            .claim(&mail, SheetKind::AddAccount)
            .expect_err("a second while the first is open");
        assert!(matches!(again, fdo::Error::LimitsExceeded(_)), "{again:?}");
        // Another kind, or another app, is its own.
        let _choose = open.claim(&mail, SheetKind::Choose).expect("another kind");
        let _other = open
            .claim(&app("org.quire.Photos"), SheetKind::AddAccount)
            .expect("another app");
        drop(first);
        let _next = open
            .claim(&mail, SheetKind::AddAccount)
            .expect("free once the first ended");
    }
}

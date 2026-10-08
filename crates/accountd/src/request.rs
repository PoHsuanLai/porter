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
use crate::core::{Core, Host, Standing, handle_token, slug};
use crate::errors::RefusedError;
use porter_core::capability::{AgentProgram, LlmFeature};
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountsReply, AccountsRequest, AppId, AppName, CapabilityKind, DataClass, Isolation,
    LauncherSession, Need, Tokens,
};
use porter_dbus::{
    CallerRole, Details, LauncherFault, SheetKind, is_handle_token, request_path, response_of,
};
use std::collections::{BTreeSet, HashSet};
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

/// The sheets open now, by the app that asked and the kind of sheet: an app has one of each
/// kind at a time, so it cannot stack sheets on the person (rel-11). Keyed by the app, not the
/// connection, so a second connection of the same app is the same asker.
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenSheets(Arc<Mutex<HashSet<(AppId, SheetKind)>>>);

/// One open sheet, held by its task; dropping it (the sheet answered, was closed, or its caller
/// left) frees the app to ask again.
#[derive(Debug)]
pub(crate) struct SheetClaim {
    open: OpenSheets,
    key: (AppId, SheetKind),
}

impl OpenSheets {
    /// A claim on a `kind` sheet for `app`, or `LimitsExceeded` while it has one open: no
    /// `Refusal` says "you have one open already" (each is about the account or the person), so
    /// the method answers the bus's own error and no sheet or Request object is made.
    pub(crate) fn claim(&self, app: &AppId, kind: SheetKind) -> Result<SheetClaim, RefusedError> {
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
            false => Err(RefusedError::busy(
                "this app already has a sheet of this kind open",
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
        let caller = self.identify(header, Standing::Acting).await?;
        let app = caller.app.clone();
        let shell = matches!(caller.role, CallerRole::SheetHost | CallerRole::Settings);
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        let path = self.request_object_path(&sender, options)?;
        let claim = self.open_sheets.claim(&app, kind)?;
        let core = Arc::clone(self);
        let run = async move {
            // Held until the sheet ends, however it ends (answered, closed, its caller gone).
            let _claim = claim;
            let reply = match request {
                // An agent signs itself in: the sheet waits for its launcher's report.
                AccountsRequest::Reauthenticate { account, window } if core.is_agent(&account) => {
                    core.host
                        .login_agent(&app, &account, window, shell, &core.launchers)
                        .await
                }
                // The shell signs any account in again; an app needs its grant.
                AccountsRequest::Reauthenticate { account, window } if shell => {
                    core.host.reauthenticate_any(&app, &account, window).await
                }
                request => core.host.handle(&app, request).await,
            };
            core.publish().await;
            reply
        };
        start(connection, sender, path, kind, run).await
    }

    /// The path of the Request object for a sheet `sender` starts: its `handle_token` if the
    /// options name one, else one minted here.
    fn request_object_path(
        &self,
        sender: &UniqueName<'_>,
        options: &Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let token = match handle_token(options) {
            Some(token) if is_handle_token(&token) => token,
            Some(token) => {
                return Err(RefusedError::invalid(format!(
                    "handle_token `{token}` is not a path segment"
                )));
            }
            None => format!("accountd_{}", self.minted.fetch_add(1, Ordering::Relaxed)),
        };
        request_path(sender.as_str(), &token)
            .and_then(|path| ObjectPath::try_from(path).ok())
            .map(OwnedObjectPath::from)
            .ok_or_else(|| RefusedError::invalid("no request path for this sender"))
    }

    /// `Peer.RequestAgentGrant`: the consent sheet for the key of the agent program `program`,
    /// for the launcher that registered it. The grant is held by the app
    /// `org.quire.Agent.<program>` (the audience P4 and P2 look for), for the Llm kind
    /// (`kind` must say so) and interactive use. `session` is empty, or a session this
    /// connection holds open, which lets the sheet offer "This session only". A session grant
    /// the sheet makes for a session that closed while it was open is removed, and the request
    /// ends dismissed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn agent_grant_sheet(
        self: &Arc<Self>,
        header: &Header<'_>,
        connection: &Connection,
        program: &str,
        kind: &str,
        class: &str,
        session: &str,
        window: ParentWindow,
        options: &Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let owner = self.launcher_of(header).await?;
        let program = AgentProgram::parse(program).map_err(RefusedError::invalid)?;
        if slug::<CapabilityKind>(kind)? != CapabilityKind::Llm {
            return Err(RefusedError::invalid(
                "an agent's grant is for the llm kind (its key)",
            ));
        }
        let class: DataClass = slug(class)?;
        let session = match session.is_empty() {
            true => None,
            false => {
                let session = LauncherSession::parse(session).map_err(RefusedError::invalid)?;
                if !self.launchers.session_open(&owner, &session) {
                    return Err(RefusedError::launcher(
                        LauncherFault::UnknownSession,
                        "this connection holds no such session open",
                    ));
                }
                Some(session)
            }
        };
        if !self.launchers.holds(&owner, &program) {
            return Err(RefusedError::launcher(
                LauncherFault::NotRegistered,
                "this connection has not registered that program",
            ));
        }
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_owned();
        let path = self.request_object_path(&sender, options)?;
        let agent = AppId {
            name: AppName::parse(&format!("org.quire.Agent.{program}"))
                .map_err(RefusedError::invalid)?,
            isolation: Isolation::Unsandboxed,
        };
        let claim = self.open_sheets.claim(&agent, SheetKind::Choose)?;
        let core = Arc::clone(self);
        let run = async move {
            let _claim = claim;
            let reply = core
                .host
                .choose_for_agent(
                    &agent,
                    llm_need(),
                    (class, Usage::Interactive),
                    &window,
                    session.as_ref(),
                )
                .await;
            let reply = core.settle_session(&owner, session.as_ref(), reply).await;
            core.publish().await;
            reply
        };
        start(connection, sender, path, SheetKind::Choose, run).await
    }

    /// A session grant the sheet made while its session closed must not stay: the closing
    /// removed the grants then, and this one came after.
    async fn settle_session(
        &self,
        owner: &str,
        session: Option<&LauncherSession>,
        reply: AccountsReply,
    ) -> AccountsReply {
        match (&reply, session) {
            (AccountsReply::Chosen(chosen), Some(session))
                if !self.launchers.session_open(owner, session) =>
            {
                let removed = self.host.end_session_grants(Some(session)).await;
                match removed.contains(&chosen.grant) {
                    true => AccountsReply::Refused(Refusal::Dismissed),
                    false => reply,
                }
            }
            _ => reply,
        }
    }
}

/// What an agent program's key is: a language model, chat, any context.
fn llm_need() -> Need {
    Need::Llm(LlmNeed {
        features: BTreeSet::from([LlmFeature::Chat]),
        context: Tokens(0),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            again.error_name(),
            "org.freedesktop.DBus.Error.LimitsExceeded"
        );
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

//! The agent launcher's side of inferd: `org.quire.Inference1.Launcher` at `/org/quire/Inference1`
//! (moved from accountd's `Peer` and `Tokens`, accounts split Phase B).
//!
//! Whoever uses a credential holds it: the agent launcher (docket-acp) registers the programs it
//! launches, opens sessions, asks for the consent sheet of an agent's key (`RequestAgentKey`, a
//! Request object answered with a permit), reports an agent's login or logout, and is handed an
//! API key for a process it spawns (`IssueProcessCredential`, a sealed memfd or a 0600 file under
//! the runtime directory). Only a connection the caller table knows as the agent launcher may call.
//!
//! The permits are the grants of the hosted `porter_service::AccountService` (brief R5);
//! [`LauncherHost`] is that service as the launcher uses it. [`serve_launcher`] puts the object on
//! a connection and returns the [`Launchers`] roster, which the key host keeps for its agent
//! sign-in rows and tells ([`Launchers::registry_changed`]) when it changes the registry.

mod credentials;
mod errors;
mod host;
mod object;
mod roster;
mod shared;
mod sheet;

pub use errors::RefusedError;
pub use host::LauncherHost;
pub use roster::{DEFAULT_BOUND, DEFAULT_TICK, Launchers, LoginTiming, SignOutNews};

use self::credentials::Credentials;
use self::object::LauncherObject;
use self::shared::{Changed, Core};
use crate::peers::Peers;
use porter_dbus::accounts::ServiceRoot;
use porter_dbus::inference::INFERENCE_PATH;
use porter_dbus::inference::LauncherServed;
use porter_service_core::audit::AuditSink;
use porter_sheets::{OpenSheets, RequestObjects};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;

/// How the launcher is served: where a `tmpfs_file` credential is written, the timing of a login
/// request, and who is told of a change.
#[derive(Clone)]
#[non_exhaustive]
pub struct LauncherConfig {
    /// The directory a `tmpfs_file` credential is written under (`<dir>/<credential id>/key`,
    /// 0700 and 0600; the credentials an earlier inferd left there are cleared at start).
    pub runtime_dir: PathBuf,
    /// The clock and bound of a request to a launcher, and where the audit lines go.
    pub timing: LoginTiming,
    changed: Option<Changed>,
}

impl LauncherConfig {
    /// A launcher writing its credential files under `runtime_dir`, with the default timing.
    pub fn new(runtime_dir: PathBuf) -> Self {
        Self {
            runtime_dir,
            timing: LoginTiming::default(),
            changed: None,
        }
    }

    /// The same config with `timing`.
    #[must_use]
    pub fn with_timing(mut self, timing: LoginTiming) -> Self {
        self.timing = timing;
        self
    }

    /// The same config recording the launcher's audit lines to `audit`.
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.timing = self.timing.with_audit(audit);
        self
    }

    /// The same config calling `changed` after the launcher changed the registry (an agent's
    /// state, a session's permits), so the key host can tell Settings.
    #[must_use]
    pub fn on_change(mut self, changed: impl Fn() + Send + Sync + 'static) -> Self {
        self.changed = Some(Arc::new(changed));
        self
    }
}

impl std::fmt::Debug for LauncherConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LauncherConfig")
            .field("runtime_dir", &self.runtime_dir)
            .field("timing", &self.timing)
            .field("changed", &self.changed.as_ref().map(|_| "<set>"))
            .finish()
    }
}

/// Serves `org.quire.Inference1.Launcher` on `connection` over `host`, answering to the agent
/// launcher `callers` names. The host is told which programs have a launcher; the permits of a
/// session nobody can end (a session permit a file holds from an inferd that died) are removed.
/// Returns the roster of launchers.
pub async fn serve_launcher<H: LauncherHost, P: Peers>(
    connection: &Connection,
    host: Arc<H>,
    callers: Arc<P>,
    config: LauncherConfig,
) -> zbus::Result<Launchers> {
    // No launcher session is open in a starting inferd, so a session permit the store holds is
    // one nobody can end.
    host.end_session_grants(None).await;
    let launchers = Launchers::new(connection.clone(), config.timing);
    host.use_launcher_roster(launchers.roster());
    let core = Arc::new(Core {
        host,
        peers: callers,
        launchers: launchers.clone(),
        credentials: Credentials::new(Some(&config.runtime_dir)),
        requests: RequestObjects::new(connection.clone(), ServiceRoot::Inference),
        open_sheets: OpenSheets::default(),
        changed: config.changed,
    });
    let sweeping = Arc::downgrade(&core);
    launchers.on_sweep(Box::new(move || {
        let core = sweeping.upgrade();
        Box::pin(async move {
            if let Some(core) = core {
                core.sweep().await;
            }
        })
    }));
    connection
        .object_server()
        .at(
            INFERENCE_PATH,
            LauncherServed(LauncherObject(Arc::clone(&core))),
        )
        .await?;
    watch(connection, &core).await?;
    Ok(launchers)
}

/// Starts the task that frees a departed connection's programs, sessions and credentials.
async fn watch<H: LauncherHost, P: Peers>(
    connection: &Connection,
    core: &Arc<Core<H, P>>,
) -> zbus::Result<()> {
    let mut owners = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await?;
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await
        {
            let Some(core) = core.upgrade() else { return };
            if let Ok(args) = signal.args()
                && args.new_owner().is_none()
            {
                core.left(args.name().as_str()).await;
            }
        }
    });
    Ok(())
}

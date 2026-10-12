//! A hand-written inferd for the client's key and launcher tests: `org.quire.Inference1.Keys` and
//! `org.quire.Inference1.Launcher` on inferd's name, with Request objects under
//! `/org/quire/Inference1/request`. How its sheets end is a `Sheets` the test sets, so the
//! client's paths all run: an answer that beats the method's reply, a forged one from a
//! connection that does not own the name, a sheet nobody answers, a refusal. It records what it
//! was asked.

use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountLabel, AccountsReply, Candidate, GrantId, ProviderId, Restriction, Subject,
};
use porter_dbus::{
    Details, REQUEST_INTERFACE, ServiceRoot, SheetKind, request_path_in, response_of,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use zbus::message::Header;
use zbus::zvariant::{Fd, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, DBusError};

/// The key every credential this fake issues holds.
pub const KEY: &str = "sk-ant-test-0123456789abcdef";

/// How the fake's sheets end.
#[derive(Debug, Clone)]
pub enum Sheets {
    /// The person says yes: the chosen key, with this permit.
    Permit(&'static str),
    /// The person adds this account.
    Added(&'static str),
    /// The person tried to add an account they already have.
    AlreadyAdded(&'static str),
    /// The sheet ends with this refusal.
    Refuse(Refusal),
    /// Nobody answers: the sheet stays open until it is closed.
    Hang,
}

/// What the fake was asked, shared with the test.
#[derive(Debug, Default)]
pub struct Seen {
    /// `(class, usage, parent_window)` of each `RequestKey`.
    pub key_requests: Vec<(String, String, String)>,
    /// `(provider_hint)` of each `AddAccount`.
    pub add_requests: Vec<String>,
    /// How many Request objects were closed by their caller.
    pub closed: usize,
    /// The programs registered, and the connection that registered them.
    pub registered: Vec<String>,
    /// `(request, outcome, reason)` of each report, login and logout alike.
    pub reports: Vec<(String, String, String)>,
    /// The sessions begun and not ended.
    pub sessions: Vec<String>,
    /// `(program, class, session)` of each `RequestAgentKey`.
    pub agent_key_requests: Vec<(String, String, String)>,
    /// `(permit, program, target)` of each `IssueProcessCredential`.
    pub issued: Vec<(String, String, String)>,
    /// The ids of the credentials revoked by their launcher.
    pub revoked: Vec<String>,
}

#[derive(Debug)]
struct Shared {
    seen: Mutex<Seen>,
    sheets: Mutex<Sheets>,
    stranger: Mutex<Option<Connection>>,
    runtime: PathBuf,
    next: AtomicU32,
    launcher: Mutex<Option<String>>,
}

/// The fake daemon: build it, `serve` it, and read `seen`.
#[derive(Debug, Clone)]
pub struct FakeInferd {
    shared: Arc<Shared>,
}

/// What the fake answers a call with when it refuses.
#[derive(Debug, DBusError)]
#[zbus(prefix = "org.quire.Inference1.Error")]
pub enum Fault {
    #[zbus(error)]
    ZBus(zbus::Error),
    UnknownRequest(String),
    UnknownCredential(String),
    OnceGrant(String),
    UnknownGrant(String),
}

pub fn candidate(permit: &str) -> Candidate {
    Candidate::new(
        AccountId::parse("anthropic").expect("id"),
        AccountLabel("Anthropic".into()),
        ProviderId::parse("anthropic").expect("provider"),
        Subject::Account,
        porter_fake::storage_account()
            .capabilities
            .iter()
            .find_map(|claim| match &claim.offer {
                porter_core::Offer::Present(capability) => Some(capability.clone()),
                _ => None,
            })
            .expect("a capability"),
        Restriction::none(),
        GrantId::parse(permit).expect("a permit id"),
    )
}

impl FakeInferd {
    /// A fake whose credentials are files under `runtime`, with its sheets ending as `sheets`.
    pub fn new(runtime: PathBuf, sheets: Sheets) -> Self {
        Self {
            shared: Arc::new(Shared {
                seen: Mutex::new(Seen::default()),
                sheets: Mutex::new(sheets),
                stranger: Mutex::new(None),
                runtime,
                next: AtomicU32::new(0),
                launcher: Mutex::new(None),
            }),
        }
    }

    /// Has `stranger`, a connection that does not own the name, send a forged dismissal of every
    /// sheet before the real answer.
    pub fn forged_by(self, stranger: Connection) -> Self {
        *self.shared.stranger.lock().expect("lock") = Some(stranger);
        self
    }

    /// What it was asked so far.
    pub fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.shared.seen.lock().expect("lock"))
    }

    /// Serves Keys and Launcher on `connection` under inferd's name and path.
    pub async fn serve(&self, connection: &Connection) {
        let server = connection.object_server();
        server
            .at(
                porter_dbus::INFERENCE_PATH,
                Keys {
                    shared: Arc::clone(&self.shared),
                },
            )
            .await
            .expect("serve Keys");
        server
            .at(
                porter_dbus::INFERENCE_PATH,
                Launcher {
                    shared: Arc::clone(&self.shared),
                },
            )
            .await
            .expect("serve Launcher");
        connection
            .request_name(porter_dbus::INFERENCE_BUS)
            .await
            .expect("own the name");
    }

    /// Sends the registered launcher (alone) a signal of three strings.
    pub async fn tell_launcher(
        &self,
        connection: &Connection,
        signal: &str,
        body: (&str, &str, &str),
    ) {
        let to = self
            .shared
            .launcher
            .lock()
            .expect("lock")
            .clone()
            .expect("a launcher registered");
        connection
            .emit_signal(
                Some(to),
                porter_dbus::INFERENCE_PATH,
                "org.quire.Inference1.Launcher",
                signal,
                &body,
            )
            .await
            .expect("signal");
    }

    /// Sends the registered launcher (alone) `ProcessCredentialRevoked`.
    pub async fn revoke_for_launcher(&self, connection: &Connection, id: &str, reason: &str) {
        let to = self
            .shared
            .launcher
            .lock()
            .expect("lock")
            .clone()
            .expect("a launcher registered");
        connection
            .emit_signal(
                Some(to),
                porter_dbus::INFERENCE_PATH,
                "org.quire.Inference1.Launcher",
                "ProcessCredentialRevoked",
                &(id, reason),
            )
            .await
            .expect("signal");
    }
}

/// A Request object: counts the `Close` it gets.
struct RequestObject {
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.quire.Accounts1.Request")]
impl RequestObject {
    async fn close(&self) {
        self.shared.seen.lock().expect("lock").closed += 1;
    }
}

/// Makes the Request object for the call that carried `options`, and ends it as the fake's
/// `Sheets` say, with `permit_or_account` the answer of a `Done` sheet.
async fn sheet(
    shared: &Arc<Shared>,
    header: &Header<'_>,
    connection: &Connection,
    options: &Details,
    kind: SheetKind,
    reply: impl FnOnce(&Sheets) -> Option<AccountsReply>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let sender = header.sender().expect("a sender").to_owned();
    let token = String::try_from(options["handle_token"].try_clone().expect("clone"))
        .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
    let path = request_path_in(ServiceRoot::Inference, sender.as_str(), &token)
        .ok_or_else(|| zbus::fdo::Error::InvalidArgs("handle_token".into()))?;
    let owned = OwnedObjectPath::try_from(path.clone())
        .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    connection
        .object_server()
        .at(
            path.as_str(),
            RequestObject {
                shared: Arc::clone(shared),
            },
        )
        .await?;
    let answer = reply(&shared.sheets.lock().expect("lock").clone());
    let stranger = shared.stranger.lock().expect("lock").clone();
    if let (Some(stranger), Some(_)) = (stranger, &answer) {
        let forged = response_of(kind, &AccountsReply::Refused(Refusal::Dismissed));
        stranger
            .emit_signal(
                Some(sender.clone()),
                path.as_str(),
                REQUEST_INTERFACE,
                "Response",
                &(forged.code.to_wire(), forged.results),
            )
            .await?;
    }
    if let Some(answer) = answer {
        let genuine = response_of(kind, &answer);
        // Before the reply: the signal is on the wire ahead of the method return.
        connection
            .emit_signal(
                Some(sender),
                path.as_str(),
                REQUEST_INTERFACE,
                "Response",
                &(genuine.code.to_wire(), genuine.results),
            )
            .await?;
    }
    Ok(owned)
}

fn choose_reply(sheets: &Sheets) -> Option<AccountsReply> {
    match sheets {
        Sheets::Permit(permit) => Some(AccountsReply::Chosen(candidate(permit))),
        Sheets::Refuse(refusal) => Some(AccountsReply::Refused(*refusal)),
        _ => None,
    }
}

struct Keys {
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.quire.Inference1.Keys")]
impl Keys {
    async fn request_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        class: String,
        usage: String,
        parent_window: String,
        options: Details,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.shared
            .seen
            .lock()
            .expect("lock")
            .key_requests
            .push((class, usage, parent_window));
        sheet(
            &self.shared,
            &header,
            connection,
            &options,
            SheetKind::Choose,
            choose_reply,
        )
        .await
    }

    async fn add_account(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        provider_hint: String,
        _parent_window: String,
        options: Details,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.shared
            .seen
            .lock()
            .expect("lock")
            .add_requests
            .push(provider_hint);
        sheet(
            &self.shared,
            &header,
            connection,
            &options,
            SheetKind::AddAccount,
            |sheets| match sheets {
                Sheets::Added(id) => Some(AccountsReply::Added(AccountId::parse(id).expect("id"))),
                Sheets::AlreadyAdded(id) => Some(AccountsReply::AlreadyAdded(
                    AccountId::parse(id).expect("id"),
                )),
                Sheets::Refuse(refusal) => Some(AccountsReply::Refused(*refusal)),
                _ => None,
            },
        )
        .await
    }
}

struct Launcher {
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.quire.Inference1.Launcher")]
impl Launcher {
    async fn register_launcher(
        &self,
        #[zbus(header)] header: Header<'_>,
        programs: Vec<String>,
    ) -> zbus::fdo::Result<()> {
        *self.shared.launcher.lock().expect("lock") =
            header.sender().map(|sender| sender.to_string());
        self.shared
            .seen
            .lock()
            .expect("lock")
            .registered
            .extend(programs);
        Ok(())
    }

    async fn set_agent_state(&self, _account: String, _state: String) {}

    async fn begin_session(&self, session: String) {
        self.shared
            .seen
            .lock()
            .expect("lock")
            .sessions
            .push(session);
    }

    async fn end_session(&self, session: String) {
        self.shared
            .seen
            .lock()
            .expect("lock")
            .sessions
            .retain(|open| *open != session);
    }

    async fn request_agent_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        program: String,
        class: String,
        session: String,
        _parent_window: String,
        options: Details,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.shared
            .seen
            .lock()
            .expect("lock")
            .agent_key_requests
            .push((program, class, session));
        sheet(
            &self.shared,
            &header,
            connection,
            &options,
            SheetKind::Choose,
            choose_reply,
        )
        .await
    }

    async fn report_agent_login(
        &self,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), Fault> {
        self.report(request, outcome, reason)
    }

    async fn report_agent_logout(
        &self,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), Fault> {
        self.report(request, outcome, reason)
    }

    /// A credential of the key `KEY`: a memfd-like anonymous file for `memfd`, a 0600 file under
    /// the fake's runtime directory for `tmpfs_file`. The permits `g-once` and `g-gone` are
    /// refused as inferd refuses them.
    async fn issue_process_credential(
        &self,
        permit: String,
        program: String,
        target: String,
    ) -> Result<(String, OwnedValue), Fault> {
        match permit.as_str() {
            "g-once" => return Err(Fault::OnceGrant(String::new())),
            "g-gone" => return Err(Fault::UnknownGrant(String::new())),
            _ => {}
        }
        let id = format!("cred-{}", self.shared.next.fetch_add(1, Ordering::Relaxed));
        self.shared
            .seen
            .lock()
            .expect("lock")
            .issued
            .push((permit, program, target.clone()));
        let path = self.shared.runtime.join(&id);
        let io = |e: std::io::Error| Fault::ZBus(zbus::Error::Failure(e.to_string()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io)?;
        std::io::Write::write_all(&mut file, KEY.as_bytes()).map_err(io)?;
        let handle = if target == "memfd" {
            // Anonymous: nothing is left on disk, as with a memfd.
            std::fs::remove_file(&path).map_err(io)?;
            OwnedValue::try_from(Fd::from(std::os::fd::OwnedFd::from(file)))
        } else {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
            OwnedValue::try_from(Value::from(path.to_string_lossy().into_owned()))
        }
        .map_err(|e| Fault::ZBus(zbus::Error::Failure(e.to_string())))?;
        Ok((id, handle))
    }

    async fn revoke_process_credential(&self, id: String) -> Result<(), Fault> {
        let mut seen = self.shared.seen.lock().expect("lock");
        if seen.revoked.contains(&id) {
            return Err(Fault::UnknownCredential(String::new()));
        }
        seen.revoked.push(id.clone());
        let path = self.shared.runtime.join(&id);
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}

impl Launcher {
    /// A report is heard once; the same request again is unknown.
    fn report(&self, request: String, outcome: String, reason: String) -> Result<(), Fault> {
        let mut seen = self.shared.seen.lock().expect("lock");
        if seen.reports.iter().any(|(said, _, _)| *said == request) {
            return Err(Fault::UnknownRequest(String::new()));
        }
        seen.reports.push((request, outcome, reason));
        Ok(())
    }
}

//! `org.quire.Accounts1.Peer`: what inferd and syncd ask on behalf of an app (porter PLAN G3).
//! Only a connection whose caller role is `PorterDaemon` may call; every other sender is
//! `AccessDenied`. The app is named by the daemon from its own connection, never by the app.
//!
//! `SetAgentState`, `RegisterLauncher`, `ReportAgentLogin`, `ReportAgentLogout`, `BeginSession`,
//! `EndSession` and `RequestAgentGrant` are the methods the agent launcher (role `AgentLauncher`)
//! may call, and the only ones it may call; the signals `AgentLoginRequested` and
//! `AgentLogoutRequested` are sent to it alone (`launchers`).
//!
//! `Verdicts`, `ResolveKey` (a sealed memfd of an API key) and `ReportLocal` (a probed local
//! runtime becoming an account, or going offline) are served.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing, slug, window};
use crate::errors::RefusedError;
use crate::keys::{sealed_key, usable};
use crate::launchers::Ask;
use porter_core::capability::AgentProgram;
use porter_core::consent::{Decision, GrantKey, Verdict, decide_key};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountState, AgentState, CapabilityKind, Claim, GrantId, LauncherSession,
    LoginOutcome, LoginRequestId, ProviderId,
};
use porter_core::{AppId, AppName, Isolation, Match, Offer, SpaceScope, matches};
use porter_dbus::{AppArg, CallerRole, Details, NeedArg, VerdictArg, need_from_dbus};
use porter_service::{AgentFault, LocalFault, LoginEnd};
use std::sync::Arc;
use zbus::Connection;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

/// The peer object at the accountd path.
#[derive(Debug)]
pub(crate) struct Peer<H, C>(Arc<Core<H, C>>);

impl<H, C> Peer<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// The app a daemon names: its reverse-DNS name and isolation slug.
fn app_of(arg: AppArg) -> Result<AppId, RefusedError> {
    let (name, isolation) = arg;
    Ok(AppId {
        name: AppName::parse(&name).map_err(RefusedError::invalid)?,
        isolation: slug::<Isolation>(&isolation)?,
    })
}

fn text(value: &str) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(value.to_owned())).ok()
}

#[zbus::interface(name = "org.quire.Accounts1.Peer")]
impl<H: Host, C: Callers> Peer<H, C> {
    async fn verdicts(
        &self,
        #[zbus(header)] header: Header<'_>,
        app: AppArg,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> Result<Vec<VerdictArg>, RefusedError> {
        let caller = self.0.identify(&header, crate::core::Standing::Any).await?;
        if caller.role != CallerRole::PorterDaemon {
            return Err(RefusedError::access_denied(
                "only a porter daemon may ask the peer interface",
            ));
        }
        let app = app_of(app)?;
        let need = need_from_dbus(need).map_err(RefusedError::invalid)?;
        let (class, usage) = (slug(&class)?, slug(&usage)?);
        let registry = self.0.host.registry();
        Ok(registry
            .accounts
            .iter()
            .filter_map(|account| {
                let claim = account.capabilities.iter().find(|claim| {
                    matches!(claim.offer, Offer::Present(_))
                        && matches(&need, &claim.offer) == Match::Fits
                })?;
                let key = GrantKey {
                    app: app.clone(),
                    account: account.id.clone(),
                    kind: claim.offer.kind(),
                    class,
                    usage,
                    space: SpaceScope::Any,
                };
                let mut details = Details::new();
                // The provider file the account was made from: inferd reaches a hosted model
                // through it, so it never guesses from the account id.
                details.extend(text(account.provider.as_str()).map(|v| ("provider".to_owned(), v)));
                // What inferd's `Places` lists the account as: its label, its provider's name as
                // a person reads it (left out when the provider file is gone), and whether it
                // works now (the state slug).
                details.extend(text(&account.label.0).map(|v| ("label".to_owned(), v)));
                details.extend(
                    self.0
                        .provider_names
                        .label_of(&account.provider)
                        .and_then(text)
                        .map(|v| ("provider_label".to_owned(), v)),
                );
                details.extend(
                    text(crate::account::state_slug(account.state))
                        .map(|v| ("state".to_owned(), v)),
                );
                let word = match decide_key(&registry.grants, &key) {
                    Verdict::Granted { grant, scope } => {
                        details.extend(text(grant.as_str()).map(|v| ("grant".to_owned(), v)));
                        // The scope's word, and beside it the session a `session` scope lasts
                        // for (`GrantScope::from_words` puts them back together).
                        details.extend(text(scope.word()).map(|v| ("scope".to_owned(), v)));
                        details.extend(
                            scope
                                .session()
                                .and_then(|session| text(session.as_str()))
                                .map(|v| ("session".to_owned(), v)),
                        );
                        "granted"
                    }
                    Verdict::Denied => "denied",
                    Verdict::Ask => "ask",
                };
                Some((account.id.to_string(), word.to_owned(), details))
            })
            .collect())
    }

    /// The API key of a granted Llm account, on a sealed memfd and never a string on the bus.
    /// The grant is named by the daemon from the verdict it asked for: it must be an `Allow`, for
    /// the Llm kind (`AudienceNotGranted` otherwise), of an account that is not turned off for
    /// Llm and is not waiting to be signed in again. The release is audited, without the key.
    async fn resolve_key(
        &self,
        #[zbus(header)] header: Header<'_>,
        grant: String,
    ) -> Result<OwnedFd, RefusedError> {
        let caller = self.0.identify(&header, crate::core::Standing::Any).await?;
        if caller.role != CallerRole::PorterDaemon {
            return Err(RefusedError::access_denied(
                "only a porter daemon may ask the peer interface",
            ));
        }
        let grant = GrantId::parse(&grant).map_err(RefusedError::invalid)?;
        let desk = self
            .0
            .keys
            .as_ref()
            .ok_or_else(|| RefusedError::of(Refusal::Unavailable))?;
        let registry = self.0.host.registry();
        let held = registry
            .grants
            .iter()
            .find(|g| g.id == grant && g.decision == Decision::Allow)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        if held.key.kind != CapabilityKind::Llm {
            return Err(RefusedError::of(Refusal::AudienceNotGranted));
        }
        let account = registry
            .accounts
            .iter()
            .find(|a| a.id == held.key.account)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        usable(&registry, account).map_err(RefusedError::of)?;
        let key = desk.read(&account.id).await.map_err(RefusedError::of)?;
        let fd = sealed_key(key.expose()).map_err(|_| RefusedError::of(Refusal::Unavailable))?;
        desk.note(&held.key.app, &account.id, &grant);
        Ok(OwnedFd::from(fd))
    }

    /// A probed local runtime becoming an account of `provider` (`ollama`, `llama-cpp`,
    /// `lm-studio`): its models as `Discovered` claims, its state `ok` or `offline`. Returns the
    /// account id. The account is made on the first report and never deleted for going offline.
    async fn report_local(
        &self,
        #[zbus(header)] header: Header<'_>,
        provider: String,
        claims: Vec<(String, Details)>,
        state: String,
    ) -> Result<String, RefusedError> {
        let caller = self.0.identify(&header, crate::core::Standing::Any).await?;
        if caller.role != CallerRole::PorterDaemon {
            return Err(RefusedError::access_denied(
                "only a porter daemon may ask the peer interface",
            ));
        }
        let provider = ProviderId::parse(&provider).map_err(RefusedError::invalid)?;
        let state: AccountState = slug(&state)?;
        let models = claims
            .iter()
            .map(|(kind, fields)| claim_of(kind, fields))
            .collect::<Result<Vec<_>, _>>()?;
        let id = self
            .0
            .host
            .report_local(&provider, models, state)
            .await
            .map_err(fault)?;
        self.0.publish().await;
        Ok(id.to_string())
    }

    /// What an agent program says of its own sign-in, for the account of an agent that signs
    /// itself in. Only the launcher (`AgentLauncher`) may say it, and it may say nothing else;
    /// `PorterDaemon` may not. Porter keeps this word and nothing of the agent's login: no
    /// token, no key, no file of the agent's is read.
    async fn set_agent_state(
        &self,
        #[zbus(header)] header: Header<'_>,
        account: String,
        state: String,
    ) -> Result<(), RefusedError> {
        let caller = self.0.identify(&header, Standing::Launching).await?;
        if caller.role != CallerRole::AgentLauncher {
            return Err(RefusedError::access_denied(
                "only the agent launcher may report an agent's state",
            ));
        }
        let account = AccountId::parse(&account).map_err(RefusedError::invalid)?;
        let state: AgentState = slug(&state)?;
        let changed = self
            .0
            .host
            .set_agent_state(&account, state)
            .await
            .map_err(agent_fault)?;
        if changed {
            self.0.publish().await;
        }
        Ok(())
    }

    /// Makes the caller the launcher of these agent programs, for as long as its connection
    /// lives. One launcher per program, first wins (`AlreadyRegistered`); a program the caller
    /// holds already is a no-op, and a later call adds more.
    async fn register_launcher(
        &self,
        #[zbus(header)] header: Header<'_>,
        programs: Vec<String>,
    ) -> Result<(), RefusedError> {
        let sender = self.launcher(&header).await?;
        let programs = programs
            .iter()
            .map(|program| AgentProgram::parse(program).map_err(RefusedError::invalid))
            .collect::<Result<Vec<_>, _>>()?;
        if programs.is_empty() {
            return Err(RefusedError::invalid("no program to launch"));
        }
        self.0
            .launchers
            .register(&sender, &programs)
            .await
            .map_err(|fault| RefusedError::launcher(fault, "a launcher holds that program"))
    }

    /// Opens a launcher session for the caller (`AgentLauncher` that registered a program): the
    /// span a grant made "for this session only" lasts. It belongs to this connection; it ends
    /// at `EndSession` or when the connection leaves the bus. A session another connection
    /// holds is `SessionTaken`; one this connection holds is a no-op.
    async fn begin_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        session: String,
    ) -> Result<(), RefusedError> {
        let owner = self.launcher(&header).await?;
        let session = LauncherSession::parse(&session).map_err(RefusedError::invalid)?;
        self.0
            .launchers
            .begin_session(&owner, session)
            .map_err(|fault| RefusedError::launcher(fault, "cannot open that session"))
    }

    /// Ends a session the caller began: its session grants are removed, the process credentials
    /// under them end `session_closed` (and the caller is told, in the unicast signal) and the
    /// audit records each. A session that is not open, or is another connection's, is
    /// `UnknownSession` and nothing is ended.
    async fn end_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        session: String,
    ) -> Result<(), RefusedError> {
        let owner = self.launcher(&header).await?;
        let session = LauncherSession::parse(&session).map_err(RefusedError::invalid)?;
        self.0
            .launchers
            .end_session(&owner, &session)
            .map_err(|fault| {
                RefusedError::launcher(fault, "no such session for this connection")
            })?;
        self.0.close_session(&session, true).await;
        Ok(())
    }

    /// The consent sheet for an agent program's key (`kind` is `llm`): a Request object answered
    /// as `Manager.Choose`'s is. With `session`, an open session of this connection, the sheet
    /// also offers "This session only". See `request::agent_grant_sheet`.
    #[allow(clippy::too_many_arguments)]
    async fn request_agent_grant(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        program: String,
        kind: String,
        class: String,
        session: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        self.0
            .agent_grant_sheet(
                &header,
                connection,
                &program,
                &kind,
                &class,
                &session,
                window(&parent_window),
                &options,
            )
            .await
    }

    /// What the launcher reports of an `AgentLoginRequested`. `ready` sets the account's state
    /// ready (as `SetAgentState` does) before whoever waits is told. Only the connection that
    /// was sent the request may answer it.
    async fn report_agent_login(
        &self,
        #[zbus(header)] header: Header<'_>,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), RefusedError> {
        self.report(&header, Ask::Login, &request, &outcome, &reason)
            .await
    }

    /// What the launcher reports of an `AgentLogoutRequested`. `ready` (done) sets the account's
    /// state to `needs_login`.
    async fn report_agent_logout(
        &self,
        #[zbus(header)] header: Header<'_>,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), RefusedError> {
        self.report(&header, Ask::Logout, &request, &outcome, &reason)
            .await
    }

    /// Sent to the registrant of `program` alone.
    #[zbus(signal)]
    async fn agent_login_requested(
        emitter: &SignalEmitter<'_>,
        request: &str,
        account: &str,
        program: &str,
    ) -> zbus::Result<()>;

    /// Sent to the registrant of `program` alone.
    #[zbus(signal)]
    async fn agent_logout_requested(
        emitter: &SignalEmitter<'_>,
        request: &str,
        account: &str,
        program: &str,
    ) -> zbus::Result<()>;
}

impl<H: Host, C: Callers> Peer<H, C> {
    /// The unique name of a caller that is the agent launcher; anyone else is `AccessDenied`.
    async fn launcher(&self, header: &Header<'_>) -> Result<String, RefusedError> {
        self.0.launcher_of(header).await
    }

    async fn report(
        &self,
        header: &Header<'_>,
        kind: Ask,
        request: &str,
        outcome: &str,
        reason: &str,
    ) -> Result<(), RefusedError> {
        let sender = self.launcher(header).await?;
        let request = LoginRequestId::parse(request).map_err(RefusedError::invalid)?;
        // The words are not echoed back: whatever else a launcher sent stays its own.
        let outcome = LoginOutcome::from_wire(outcome, reason)
            .map_err(|_| RefusedError::invalid("not an outcome of a login"))?;
        let pending = self.0.launchers.take(&sender, &request, kind)?;
        // The state first, so whoever waits finds the account as the report left it.
        let state = match (kind, outcome) {
            (Ask::Login, LoginOutcome::Ready) => Some(AgentState::Ready),
            (Ask::Logout, LoginOutcome::Ready) => Some(AgentState::NeedsLogin),
            _ => None,
        };
        let saved = match state {
            Some(state) => self.0.host.set_agent_state(&pending.account, state).await,
            None => Ok(false),
        };
        if saved == Ok(true) {
            self.0.publish().await;
        }
        pending.end(LoginEnd::Reported(outcome));
        saved.map(|_| ()).map_err(agent_fault)
    }
}

fn agent_fault(fault: AgentFault) -> RefusedError {
    match fault {
        AgentFault::UnknownAccount => RefusedError::invalid("no such account"),
        AgentFault::NotAnAgent => {
            RefusedError::invalid("not an account of an agent that signs itself in")
        }
        AgentFault::Unavailable => RefusedError::of(Refusal::Unavailable),
    }
}

/// One reported claim: the kind's slug and the fields by name, as `Account.Capabilities` has them.
/// The slug must be the kind of the offer the fields carry.
fn claim_of(kind: &str, fields: &Details) -> Result<Claim, RefusedError> {
    let record =
        porter_dbus::from_vardict(fields).map_err(|e| RefusedError::invalid(e.to_string()))?;
    let claim: Claim = serde_json::from_value(serde_json::Value::Object(record))
        .map_err(|e| RefusedError::invalid(format!("claim: {e}")))?;
    let stated: CapabilityKind = slug(kind)?;
    match claim.offer.kind() == stated {
        true => Ok(claim),
        false => Err(RefusedError::invalid(format!(
            "claim of kind `{kind}` carries another kind"
        ))),
    }
}

fn fault(fault: LocalFault) -> RefusedError {
    match fault {
        LocalFault::Unavailable => RefusedError::of(Refusal::Unavailable),
        other => RefusedError::invalid(format!("{other:?}")),
    }
}

//! `RequestAgentKey`: the consent sheet for an agent program's key, as a Request object
//! (`porter_sheets::RequestObjects`, rooted at `ServiceRoot::Inference`).
//!
//! The permit is held by the app `org.quire.Agent.<program>` (the audience P4 and P2 look for),
//! for the Llm kind (there is no `kind` argument: it was always `llm`) and interactive use.
//! `session` is empty, or a session this connection holds open, which lets the sheet offer "This
//! session only". A session permit the sheet makes for a session that closed while it was open is
//! removed, and the request ends dismissed.

use super::errors::RefusedError;
use super::host::LauncherHost;
use super::shared::{AGENT_APP_PREFIX, Core};
use crate::peers::Peers;
use porter_capability::{AgentProgram, LlmFeature};
use porter_core::SessionToken;
use porter_core::consent::Usage;
use porter_core::{AppId, AppName, DataClass, Isolation};
use porter_dbus::Details;
use porter_dbus::accounts::sheet::SheetKind;
use porter_dbus::inference::LauncherFault;
use porter_ids::Tokens;
use porter_match::LlmNeed;
use porter_match::Need;
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};
use std::collections::BTreeSet;
use std::sync::Arc;
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;

/// A closed set's slug argument, parsed at the boundary.
pub(crate) fn slug<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RefusedError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| RefusedError::invalid(format!("`{text}`: {e}")))
}

/// What an agent program's key is: a language model, chat, any context.
fn llm_need() -> Need {
    Need::Llm(LlmNeed::new(BTreeSet::from([LlmFeature::Chat]), Tokens(0)))
}

impl<H: LauncherHost, P: Peers> Core<H, P> {
    /// The consent sheet for the key of the agent program `program`, for the launcher that
    /// registered it.
    pub(crate) async fn agent_key_sheet(
        self: &Arc<Self>,
        header: &Header<'_>,
        program: &str,
        class: &str,
        session: &str,
        window: ParentWindow,
        options: &Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        let owner = self.launcher_of(header).await?;
        let program = AgentProgram::parse(program).map_err(RefusedError::invalid)?;
        let class: DataClass = slug(class)?;
        let session = match session.is_empty() {
            true => None,
            false => {
                let session = SessionToken::parse(session).map_err(RefusedError::invalid)?;
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
        let agent = AppId {
            name: AppName::parse(&format!("{AGENT_APP_PREFIX}{program}"))
                .map_err(RefusedError::invalid)?,
            isolation: Isolation::Unsandboxed,
        };
        let claim = self
            .open_sheets
            .claim(&agent, SheetKind::Choose)
            .map_err(RefusedError::busy)?;
        let core = Arc::clone(self);
        let run = async move {
            let _claim = claim;
            let reply = core
                .host
                .choose(
                    &agent,
                    llm_need(),
                    (class, Usage::Interactive),
                    &window,
                    session.as_ref(),
                )
                .await;
            let reply = core.settle_session(&owner, session.as_ref(), reply).await;
            core.settled().await;
            reply
        };
        let path = self
            .requests
            .path(&sender, options)
            .map_err(RefusedError::failed)?;
        self.requests
            .start(sender, path, SheetKind::Choose, run)
            .await
            .map_err(RefusedError::failed)
    }

    /// A session permit the sheet made while its session closed must not stay: the closing
    /// removed the permits then, and this one came after.
    async fn settle_session(
        &self,
        owner: &str,
        session: Option<&SessionToken>,
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

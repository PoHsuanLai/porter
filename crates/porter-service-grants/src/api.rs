//! [`GrantsApi`]: what a host asks of the grants domain.

use crate::Grants;
use porter_core::consent::{Availability, Grant, Usage};
use porter_core::{AppId, Candidate, DataClass, EndpointUrl, GrantId, LlmVerdict, ProviderId};
use porter_core::{RelayPlan, SecretText, SessionToken};
use porter_match::Need;
use porter_service_core::Ports;
use porter_wire::{AccountsReply, ParentWindow, Refusal};
use std::future::Future;

/// The grants domain: who may use which account, and how a grant is given, used and ended.
pub trait GrantsApi {
    /// The accounts that fit `need` for `caller` and that `caller` holds a grant for.
    fn candidates(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate>;

    /// Whether `need` can be met for `caller`, revealing no identity: an account fits and
    /// `caller` holds a grant, an account fits and the person is yet to be asked, or none fits.
    fn availability(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability;

    /// The grants `caller` holds.
    fn grants(&self, caller: &AppId) -> Vec<Grant>;

    /// Shows the chooser and consent sheet for `need` and records the grant the person gives.
    /// `session` is the open session the request was made in, which lets the sheet offer "This
    /// session only"; with one, "Add Account…" is not offered a second route: it ends the
    /// request dismissed, since the account it would add and allow gets an `Always` grant, not
    /// the session one.
    fn choose(
        &self,
        caller: &AppId,
        need: Need,
        class_usage: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Withdraws one of `caller`'s own grants; `UnknownGrant` when `caller` holds none of that
    /// id.
    fn revoke(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Withdraws any grant, whoever holds it (Settings' "Revoke"). `UnknownGrant` when none has
    /// that id.
    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Spends `caller`'s grant when it is a `Once` one: the first use of any kind (a relay
    /// opened, a key read by the host that holds the secret) drops it, so a second use asks
    /// again. A host that reads a key itself, as inferd does, calls this once the read has
    /// succeeded.
    fn spend_once(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Removes the grants scoped to `session`, or every session grant when none is named (a
    /// starting service has no open session, so a session grant a file holds is one nobody can
    /// end). Audits each as `SessionGrantEnded`, saves, and returns the grants removed.
    fn end_session_grants(
        &self,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = Vec<GrantId>> + Send;

    /// What each account that serves language models says for `app`, for data of `class` used as
    /// `usage` says. `provider_label` gives what a person reads for a provider.
    fn llm_verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict>;

    /// The key behind an allow grant of kind `Llm`. The grant must exist and be an allow, the
    /// account must be usable (its switch for language models on, no sign-in due), and the key
    /// is read through the accounts domain. `KeyResolved` is audited, never the key, and an
    /// allow-once grant is spent only after the read succeeded.
    ///
    /// # Errors
    /// `UnknownGrant`, `AudienceNotGranted` for a grant of another kind, `Denied`,
    /// `NeedsReauth` and `Unavailable`.
    fn release_key(
        &self,
        grant: &GrantId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;

    /// The permit `app` already holds for `need`, the one given most recently, so a repeat
    /// request is answered with no sheet.
    fn existing_permit(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate>;

    /// What the relay for `OpenAuthenticated` is told: checks that `caller` holds the grant and
    /// that `endpoint` is one of the account's for the grant's kind, then reads what the relay
    /// presents. The transport makes the descriptor and runs the relay.
    fn open_authenticated(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send;

    /// What the relay for `OpenLinked` is told: checks that `caller` holds the grant and that
    /// `origin` is one its account's provider file declares for the grant's kind, then plans a
    /// relay that adds no credential. The audit line names the origin, never a link's path (a
    /// pre-authenticated URL carries its secret there).
    fn open_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send;
}

impl<E: Ports> GrantsApi for Grants<'_, E> {
    fn candidates(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate> {
        self.candidates_of(caller, need, class, usage)
    }

    fn availability(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability {
        self.availability_of(caller, need, class, usage)
    }

    fn grants(&self, caller: &AppId) -> Vec<Grant> {
        self.grants_of(caller)
    }

    fn choose(
        &self,
        caller: &AppId,
        need: Need,
        class_usage: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = AccountsReply> + Send {
        Grants::choose(self, caller, need, class_usage, window, session)
    }

    fn revoke(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        Grants::revoke(self, caller, grant)
    }

    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        Grants::revoke_grant(self, grant)
    }

    fn spend_once(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        Grants::spend_once(self, caller, grant)
    }

    fn end_session_grants(
        &self,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = Vec<GrantId>> + Send {
        Grants::end_session_grants(self, session)
    }

    fn llm_verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict> {
        self.llm_verdicts_of(app, class, usage, provider_label)
    }

    fn release_key(
        &self,
        grant: &GrantId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        Grants::release_key(self, grant)
    }

    fn existing_permit(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate> {
        self.existing_permit_of(app, need, class, usage)
    }

    fn open_authenticated(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        Grants::open_authenticated(self, caller, grant, endpoint)
    }

    fn open_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<RelayPlan, Refusal>> + Send {
        Grants::open_linked(self, caller, grant, origin)
    }
}

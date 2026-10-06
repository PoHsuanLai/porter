//! A family's sign-in (porter PLAN G4): a small conversation the host drives. The host feeds it
//! a [`SignInInput`] and gets a [`SignInStep`] back, draws what the step asks for, and repeats
//! until `Done` or `Failed`. The credential comes out only in `Done(Signed)`, which the host
//! keeps: it is never drawn, never sent to a sheet and never serialised.

use porter_core::sheet::{
    FieldSpec, Progress, Review, ServiceRow, ServiceState, SignInFault, SignInInput, UserCode,
};
use porter_core::{
    AccountId, AccountLabel, Claim, Credential, EndpointUrl, Offer, Restriction, SecretPurpose,
    ServiceEndpoint, Toggle, WebUrl,
};
use std::future::Future;

/// Why the sign-in was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInMode {
    /// A new account.
    Add,
    /// An existing account whose credential was refused or is being replaced.
    Reauthenticate {
        /// The account, so the new credential replaces the old one under the same id.
        account: AccountId,
        /// Its servers as stored, so a family that signs in again needs no discovery to find
        /// them (Nextcloud's login name and server are among them).
        endpoints: Vec<ServiceEndpoint>,
    },
}

/// What a sign-in begins with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInStart {
    /// Why.
    pub mode: SignInMode,
}

/// What a signed-in account hands the host to store. `Debug` redacts every credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// The name the account will have.
    pub label: AccountLabel,
    /// The secrets to file, each under its purpose (an app password; an OAuth refresh token;
    /// a different password for the outgoing server).
    pub credentials: Vec<(SecretPurpose, Credential)>,
    /// What the account can do, as the protocol answered.
    pub claims: Vec<Claim>,
    /// Its servers.
    pub endpoints: Vec<ServiceEndpoint>,
    /// What limits it.
    pub restriction: Restriction,
}

/// What a sign-in asks the host to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInStep {
    /// Show a form with these fields and feed the answers back.
    AskFields(Vec<FieldSpec>),
    /// Open this page in the browser, then feed `Poll` until it says more.
    OpenBrowser {
        /// The page, with its query (an authorize URL is all query).
        url: WebUrl,
    },
    /// Show this code and page (the device flow), then feed `Poll`.
    ShowCode {
        /// The code the person types.
        user_code: UserCode,
        /// The page they type it at.
        url: EndpointUrl,
    },
    /// Still waiting on the person or the provider; feed `Poll` again.
    Waiting,
    /// It found these; show them, and feed `Confirm` with the person's choices.
    Review {
        /// What the account can do.
        claims: Vec<Claim>,
        /// Its servers.
        endpoints: Vec<ServiceEndpoint>,
        /// What limits it, for the secondary lines.
        restriction: Restriction,
        /// The name the account will have.
        label: AccountLabel,
    },
    /// Signed in.
    Done(Signed),
    /// It ended without an account.
    Failed(SignInFault),
}

impl SignInStep {
    /// What the sheet is told of this step: everything but the credential.
    pub fn progress(&self) -> Progress {
        match self {
            SignInStep::AskFields(fields) => Progress::Ask(fields.clone()),
            SignInStep::OpenBrowser { url } => Progress::Browser(url.clone()),
            SignInStep::ShowCode { user_code, url } => Progress::Code {
                user_code: user_code.clone(),
                url: url.clone(),
            },
            SignInStep::Waiting => Progress::Waiting,
            SignInStep::Review {
                claims,
                endpoints,
                restriction,
                label,
            } => Progress::Review(Review {
                label: label.clone(),
                services: claims.iter().map(|c| service_row(c, restriction)).collect(),
                endpoints: endpoints.clone(),
            }),
            SignInStep::Done(_) => Progress::Done,
            SignInStep::Failed(fault) => Progress::Failed(*fault),
        }
    }
}

fn service_row(claim: &Claim, restriction: &Restriction) -> ServiceRow {
    let kind = claim.offer.kind();
    ServiceRow {
        kind,
        state: match &claim.offer {
            Offer::Present(_) => ServiceState::Offered(Toggle::On),
            Offer::Absent { reason, .. } => ServiceState::Absent(*reason),
        },
        limit: restriction
            .limits
            .iter()
            .find(|limit| limit.kind == kind)
            .map(|limit| limit.reason),
    }
}

/// One family's sign-in conversation.
pub trait SignIn: Send {
    /// Takes the host's input and says what to do next. The first input is `Start`.
    fn next(&mut self, input: SignInInput) -> impl Future<Output = SignInStep> + Send;
}

/// What revoking an account at its provider came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// The provider no longer honours the credential.
    Revoked,
    /// The provider has no API for it; the person does it on this page.
    Manual(EndpointUrl),
    /// Nothing can be done at the provider.
    Unsupported,
}

#[cfg(test)]
mod tests;

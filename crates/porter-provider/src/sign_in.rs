//! A family's sign-in (porter PLAN G4): a small conversation the host drives. The host feeds it
//! a [`SignInInput`] and gets a [`SignInStep`] back, draws what the step asks for, and repeats
//! until `Done` or `Failed`. The credential comes out only in `Done(Signed)`, which the host
//! keeps: it is never drawn, never sent to a sheet and never serialised.

use porter_core::AccountId;
use porter_core::sheet::{
    FieldSpec, Progress, Review, ServiceRow, ServiceState, SignInFault, SignInInput, UserCode,
};
use porter_core::{
    AccountLabel, CapabilityKind, Claim, Credential, EndpointUrl, Offer, Restriction,
    SecretPurpose, ServiceEndpoint, Toggle,
};
use porter_ids::WebUrl;
use std::future::Future;

/// Why the sign-in was started.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// An existing account asked for the access of its on-use rows of `kinds` (the whole of a
    /// drive), and nothing else: the browser asks for that alone, and the person reviews it in
    /// plain words before it is kept. What comes back replaces the account's credential, so it
    /// must be the same person; the host refuses another.
    Extend {
        /// The account.
        account: AccountId,
        /// Its servers as stored, whose login the answer must match.
        endpoints: Vec<ServiceEndpoint>,
        /// The kinds whose on-use access is asked.
        kinds: Vec<CapabilityKind>,
    },
}

/// The address a sign-in is for, sent to the provider so its page opens on that person and asks
/// nothing more than it must ("Continue as"). A hint, not a promise: the person may still pick
/// another account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoginHint(String);

/// Why a [`LoginHint`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginHintFault {
    /// Nothing, or only blanks.
    Empty,
    /// Not an address: no `@`, or blanks or control characters inside.
    NotAnAddress,
}

impl LoginHint {
    /// The longest address mail allows.
    const MAX_CHARS: usize = 254;

    /// The hint for this address.
    ///
    /// # Errors
    /// [`LoginHintFault`] when it is empty or does not look like an address.
    pub fn parse(address: &str) -> Result<Self, LoginHintFault> {
        let address = address.trim();
        if address.is_empty() {
            return Err(LoginHintFault::Empty);
        }
        let plain = address
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control());
        let at = address
            .find('@')
            .is_some_and(|i| i > 0 && i + 1 < address.len());
        match plain && at && address.chars().count() <= Self::MAX_CHARS {
            true => Ok(Self(address.to_owned())),
            false => Err(LoginHintFault::NotAnAddress),
        }
    }

    /// The address.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether the person already agreed to what this client asks, for this identity.
///
/// Google shows its consent page, and sends a refresh token, only for a first authorization
/// unless it is forced to; forcing it on every sign-in is the extra screen "Continue as" avoids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ConsentHistory {
    /// The first sign-in of this client for this identity, or not known: ask for consent.
    #[default]
    FirstSignIn,
    /// This client was signed in for this identity before: do not ask for consent again.
    SignedInBefore,
}

/// What a sign-in begins with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignInStart {
    /// Why.
    pub mode: SignInMode,
    /// The address to open the provider's page on, when the host knows it.
    pub login_hint: Option<LoginHint>,
    /// Whether the person agreed to this client before, for this identity.
    pub consent: ConsentHistory,
}

impl SignInStart {
    /// A sign-in begun for this reason, with no address named and a first consent.
    pub fn new(mode: SignInMode) -> Self {
        Self {
            mode,
            login_hint: None,
            consent: ConsentHistory::default(),
        }
    }

    /// Opens the provider's page on this address.
    #[must_use]
    pub fn with_login_hint(mut self, hint: LoginHint) -> Self {
        self.login_hint = Some(hint);
        self
    }

    /// Says whether this client was signed in for this identity before.
    #[must_use]
    pub fn with_consent(mut self, consent: ConsentHistory) -> Self {
        self.consent = consent;
        self
    }
}

/// What a signed-in account hands the host to store. `Debug` redacts every credential.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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

impl Signed {
    /// A signed-in account called `label`, with the secrets to file, what it can do, its servers
    /// and what limits it.
    pub fn new(
        label: AccountLabel,
        credentials: Vec<(SecretPurpose, Credential)>,
        claims: Vec<Claim>,
        endpoints: Vec<ServiceEndpoint>,
        restriction: Restriction,
    ) -> Self {
        Self {
            label,
            credentials,
            claims,
            endpoints,
            restriction,
        }
    }
}

/// What a sign-in asks the host to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
        access: match &claim.offer {
            Offer::Present(capability) => capability.access_words().map(str::to_owned),
            Offer::Absent { .. } => None,
        },
    }
}

/// One family's sign-in conversation.
pub trait SignIn: Send {
    /// Takes the host's input and says what to do next. The first input is `Start`.
    fn next(&mut self, input: SignInInput) -> impl Future<Output = SignInStep> + Send;
}

/// What revoking an account at its provider came to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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

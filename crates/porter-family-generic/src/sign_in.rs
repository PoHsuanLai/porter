//! The Generic sign-in flow, written once over a [`Lookup`]: how the servers of the account are
//! found from what the person types. A mail account asks for an address and a password and
//! finds the servers from the address (the server too, when none is published); a DAV account
//! asks for the server, a user name and a password; a provider that names its own servers asks
//! for an address and a password only. What every one does with the account it found is here:
//! the password is tried at its mail server, then it is reviewed (a new account) or finished (a
//! sign-in again).

use super::login::SharedLogin;
use crate::dns::SharedDns;
use porter_capability::Claim;
use porter_core::{AccountLabel, Credential, Restriction, SecretPurpose, ServiceEndpoint};
use porter_family_kit::Io;
use porter_family_kit::{Flow, Move};
use porter_provider::{ProviderSet, ProviderSpec, SignInMode, SignInStep, Signed};
use porter_sheet::{FieldAnswer, FieldSpec, SignInFault, SignInInput};
use std::fmt::Debug;
use std::future::Future;
use std::marker::PhantomData;

/// What a lookup reads its answer through.
#[derive(Debug)]
pub struct Seams<'a> {
    pub(super) io: &'a Io,
    pub(super) dns: &'a SharedDns,
    pub(super) providers: &'a ProviderSet,
    pub(super) spec: &'a ProviderSpec,
    pub(super) mode: &'a SignInMode,
}

/// An account whose servers were located.
#[derive(Debug)]
pub struct Located {
    pub(super) credential: Credential,
    pub(super) label: String,
    pub(super) endpoints: Vec<ServiceEndpoint>,
    pub(super) claims: Vec<Claim>,
}

impl Located {
    /// The account as the review shows it.
    pub(super) fn into_signed(self) -> Signed {
        Signed::new(
            AccountLabel(self.label),
            vec![(SecretPurpose::Password, self.credential)],
            self.claims,
            self.endpoints,
            Restriction::none(),
        )
    }
}

/// What a lookup came to.
#[derive(Debug)]
pub enum Outcome<M> {
    /// The account is known: its password is tried at its mail server, then it is reviewed or
    /// finished.
    Known(Located),
    /// Servers that only unsigned DNS names, outside the address's domain: reviewed first, and
    /// the password tried once the person confirms.
    Unseen(Signed),
    /// Ask this, and carry on from `M`.
    Ask(SignInStep, M),
    /// Ended, for this reason.
    Fail(SignInFault),
}

/// How the servers of a generic account are found.
pub trait Lookup: Clone + Debug + Default + Send + Sync + 'static {
    /// What the lookup keeps when it asks a further question; `()` when it asks none.
    type Mid: Debug + Send;

    /// The first form: the password field is an app password for a provider that takes only one
    /// (ship-5).
    fn form(spec: &ProviderSpec) -> Vec<FieldSpec>;

    /// The answers to the first form.
    fn submitted(
        &self,
        seams: &Seams<'_>,
        answers: Vec<FieldAnswer>,
    ) -> impl Future<Output = Outcome<Self::Mid>> + Send;

    /// The answers to a further question. A lookup that asks none refuses them.
    fn resumed(
        &self,
        seams: &Seams<'_>,
        mid: Self::Mid,
        answers: Vec<FieldAnswer>,
    ) -> impl Future<Output = Outcome<Self::Mid>> + Send {
        let _ = (seams, mid, answers);
        async { Outcome::Fail(SignInFault::Unreadable) }
    }
}

/// Where the conversation waits.
#[derive(Debug)]
pub enum Mid<M> {
    /// The first form is on screen.
    Asked,
    /// The lookup asked a further question.
    Lookup(M),
}

/// The sign-in flow of a generic account whose servers `L` finds.
#[derive(Debug)]
pub struct GenericFlow<L> {
    io: Io,
    dns: SharedDns,
    providers: ProviderSet,
    login: SharedLogin,
    spec: ProviderSpec,
    mode: SignInMode,
    /// Whether the review on screen is of servers only unsigned DNS named, whose password is
    /// tried once the person confirms.
    untried: bool,
    lookup: PhantomData<fn() -> L>,
}

impl<L: Lookup> GenericFlow<L> {
    pub(super) fn new(
        io: Io,
        dns: SharedDns,
        providers: ProviderSet,
        login: SharedLogin,
        spec: ProviderSpec,
        mode: SignInMode,
    ) -> Self {
        Self {
            io,
            dns,
            providers,
            login,
            spec,
            mode,
            untried: false,
            lookup: PhantomData,
        }
    }

    fn seams(&self) -> Seams<'_> {
        Seams {
            io: &self.io,
            dns: &self.dns,
            providers: &self.providers,
            spec: &self.spec,
            mode: &self.mode,
        }
    }

    /// What came of a lookup: the account is known, so its password is tried at its mail server
    /// (a wrong one, or a server that does not answer, ends the sign-in here), then it is
    /// reviewed (a new account) or finished (a sign-in again).
    async fn settle(&mut self, outcome: Outcome<L::Mid>) -> Move<Mid<L::Mid>> {
        match outcome {
            Outcome::Known(found) => {
                if let Err(fault) = self.login.check(&found.credential, &found.endpoints).await {
                    return Move::Fail(fault);
                }
                let signed = found.into_signed();
                match self.mode {
                    SignInMode::Add => {
                        self.untried = false;
                        Move::Review(signed)
                    }
                    SignInMode::Reauthenticate { .. } => Move::Done(signed),
                    // a variant a newer porter adds: the sign-in ends as unreadable
                    _ => Move::Fail(SignInFault::Unreadable),
                }
            }
            Outcome::Unseen(signed) => {
                self.untried = true;
                Move::Review(signed)
            }
            Outcome::Ask(step, mid) => Move::Next(step, Mid::Lookup(mid)),
            Outcome::Fail(fault) => Move::Fail(fault),
        }
    }
}

impl<L: Lookup> Flow for GenericFlow<L> {
    type Mid = Mid<L::Mid>;

    async fn start(&mut self) -> Move<Self::Mid> {
        self.untried = false;
        Move::Next(SignInStep::AskFields(L::form(&self.spec)), Mid::Asked)
    }

    async fn resume(&mut self, mid: Self::Mid, input: SignInInput) -> Move<Self::Mid> {
        let SignInInput::Fields(answers) = input else {
            return Move::Fail(SignInFault::Unreadable);
        };
        let lookup = L::default();
        let outcome = match mid {
            Mid::Asked => lookup.submitted(&self.seams(), answers).await,
            Mid::Lookup(mid) => lookup.resumed(&self.seams(), mid, answers).await,
        };
        self.settle(outcome).await
    }

    /// The person confirmed servers whose password was not tried yet: it is tried now, and a
    /// wrong one, or a server that does not answer, ends the sign-in.
    async fn confirm(&mut self, signed: Signed) -> Result<Signed, SignInFault> {
        if !std::mem::take(&mut self.untried) {
            return Ok(signed);
        }
        match signed.credentials.first() {
            Some((_, credential)) => self.login.check(credential, &signed.endpoints).await?,
            None => return Err(SignInFault::Unreadable),
        }
        Ok(signed)
    }
}

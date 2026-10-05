//! The Nextcloud sign-in conversation: the server (when the provider file does not name one),
//! Login Flow v2 in the browser, what the account can do, and the review.

use super::discover::{Found, Who, discover};
use super::flow::{Granted, Polled, Started, poll, start};
use crate::io::Io;
use crate::password::{parse_server, plain, text_of};
use porter_core::sheet::{FieldKind, SignInFault, SignInInput};
use porter_core::{AccountLabel, Credential, EndpointUrl, Restriction, SecretPurpose};
use porter_http::Sleep;
use porter_provider::{SignIn, SignInMode, SignInStep, Signed};
use std::time::Duration;

/// How many polls in a row may fail to reach the server before the flow is given up.
const MAX_HICCUPS: u32 = 5;

/// The sign-in conversation of a Nextcloud account.
#[derive(Debug)]
pub struct NextcloudSignIn {
    io: Io,
    mode: SignInMode,
    /// Where the provider file says the Nextcloud is (a deployment's own file), if it does.
    fixed: Option<EndpointUrl>,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Nothing asked yet.
    Fresh,
    /// The server was asked for.
    AskedServer,
    /// The person is in the browser.
    Polling(Polling),
    /// Discovery is on screen, waiting for the person's confirmation.
    Reviewing(Box<Signed>),
    /// Over, or cancelled.
    Ended,
}

#[derive(Debug)]
struct Polling {
    server: EndpointUrl,
    started: Started,
    attempt: u32,
    waited: Duration,
    hiccups: u32,
}

impl NextcloudSignIn {
    pub(super) fn new(io: Io, mode: SignInMode, fixed: Option<EndpointUrl>) -> Self {
        Self {
            io,
            mode,
            fixed,
            state: State::Fresh,
        }
    }

    fn ask_server() -> SignInStep {
        SignInStep::AskFields(vec![plain(FieldKind::Server)])
    }

    fn failed(&mut self, fault: SignInFault) -> SignInStep {
        self.state = State::Ended;
        SignInStep::Failed(fault)
    }

    /// Starts the flow at `server` and sends the person to its page.
    async fn begin(&mut self, server: EndpointUrl) -> SignInStep {
        match start(&self.io, &server).await {
            Ok(started) => {
                let login = started.login.clone();
                self.state = State::Polling(Polling {
                    server,
                    started,
                    attempt: 0,
                    waited: Duration::ZERO,
                    hiccups: 0,
                });
                SignInStep::OpenBrowser { url: login }
            }
            Err(fault) => self.failed(fault),
        }
    }

    /// One poll: waits its turn, asks, and says what came of it.
    async fn poll(&mut self, mut polling: Polling) -> SignInStep {
        let pacing = self.io.pacing;
        if polling.waited >= pacing.limit {
            return self.failed(SignInFault::TimedOut);
        }
        let wait = pacing.wait(polling.attempt);
        self.io.sleep.sleep(wait).await;
        polling.attempt += 1;
        polling.waited += wait;
        match poll(&self.io, &polling.started).await {
            Polled::Waiting => {
                polling.hiccups = 0;
                self.state = State::Polling(polling);
                SignInStep::Waiting
            }
            Polled::Hiccup if polling.hiccups + 1 < MAX_HICCUPS => {
                polling.hiccups += 1;
                self.state = State::Polling(polling);
                SignInStep::Waiting
            }
            Polled::Hiccup => self.failed(SignInFault::Unreachable),
            Polled::Failed(fault) => self.failed(fault),
            Polled::Granted(granted) => self.granted(polling.server, granted).await,
        }
    }

    /// The person approved: find out what the account can do, then review (a new account) or
    /// finish (a sign-in again, which only replaces the password).
    async fn granted(&mut self, dialled: EndpointUrl, granted: Granted) -> SignInStep {
        let server = granted.server.unwrap_or(dialled);
        let who = Who {
            server,
            login: granted.login,
            password: granted.password,
        };
        let found = match discover(&self.io, &who).await {
            Ok(found) => found,
            Err(fault) => return self.failed(fault),
        };
        let signed = signed(&who, found);
        match self.mode {
            SignInMode::Add => {
                let step = SignInStep::Review {
                    claims: signed.claims.clone(),
                    endpoints: signed.endpoints.clone(),
                    restriction: signed.restriction.clone(),
                    label: signed.label.clone(),
                };
                self.state = State::Reviewing(Box::new(signed));
                step
            }
            SignInMode::Reauthenticate { .. } => {
                self.state = State::Ended;
                SignInStep::Done(signed)
            }
        }
    }
}

fn signed(who: &Who, found: Found) -> Signed {
    Signed {
        label: AccountLabel(format!("{}@{}", who.login.0, host_of(&who.server))),
        credentials: vec![(
            SecretPurpose::Password,
            Credential::Password(who.password.clone()),
        )],
        claims: found.claims,
        endpoints: found.endpoints,
        restriction: Restriction::none(),
    }
}

/// The host (and a port that is not the scheme's own) the account is shown by.
fn host_of(server: &EndpointUrl) -> String {
    let origin = server.origin();
    match origin.port == origin.scheme.default_port() {
        true => origin.host,
        false => format!("{}:{}", origin.host, origin.port),
    }
}

impl SignIn for NextcloudSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let state = std::mem::replace(&mut self.state, State::Ended);
        match (state, input) {
            (_, SignInInput::Cancel) => SignInStep::Failed(SignInFault::Cancelled),
            // Begin, and begin again after a step back.
            (_, SignInInput::Start) => match self.fixed.clone() {
                Some(server) => self.begin(server).await,
                None => {
                    self.state = State::AskedServer;
                    Self::ask_server()
                }
            },
            (State::AskedServer, SignInInput::Fields(answers)) => {
                match text_of(&answers, FieldKind::Server).and_then(|t| parse_server(&t)) {
                    Some(server) => self.begin(server).await,
                    None => self.failed(SignInFault::Unreadable),
                }
            }
            (State::Polling(polling), SignInInput::Poll) => self.poll(polling).await,
            (State::Reviewing(signed), SignInInput::Confirm(_)) => SignInStep::Done(*signed),
            _ => self.failed(SignInFault::Unreadable),
        }
    }
}

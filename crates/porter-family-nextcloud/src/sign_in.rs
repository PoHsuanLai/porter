//! The Nextcloud sign-in flow: the server (when the provider file does not name one), Login Flow
//! v2 in the browser, what the account can do, and the review.

use super::discover::{Found, Who, discover};
use super::flow::{poll, start};
use nextcloud_proto::flow::{Granted, Polled, Started};
use nextcloud_proto::paths::server_of;
use porter_core::{AccountLabel, Credential, EndpointUrl, Restriction, SecretPurpose};
use porter_family_kit::Io;
use porter_family_kit::password::{plain, text_of};
use porter_family_kit::{Flow, Move};
use porter_http::Sleep;
use porter_provider::{SignInMode, SignInStep, Signed};
use porter_sheet::{FieldKind, SignInFault, SignInInput};
use std::time::Duration;

/// How many polls in a row may fail to reach the server before the flow is given up.
const MAX_HICCUPS: u32 = 5;

/// The sign-in flow of a Nextcloud account.
#[derive(Debug)]
pub struct NextcloudFlow {
    io: Io,
    mode: SignInMode,
    /// Where the provider file says the Nextcloud is (a deployment's own file), if it does.
    fixed: Option<EndpointUrl>,
}

/// Where the conversation waits.
#[derive(Debug)]
pub enum Mid {
    /// The server was asked for.
    AskedServer,
    /// The person is in the browser.
    Polling(Polling),
}

/// A Login Flow v2 being polled.
#[derive(Debug)]
pub struct Polling {
    server: EndpointUrl,
    started: Started,
    attempt: u32,
    waited: Duration,
    hiccups: u32,
}

impl NextcloudFlow {
    pub(super) fn new(io: Io, mode: SignInMode, fixed: Option<EndpointUrl>) -> Self {
        Self { io, mode, fixed }
    }

    /// The server to start at without asking: the provider file's, or the account's own when
    /// it signs in again.
    fn known_server(&self) -> Option<EndpointUrl> {
        let held = match &self.mode {
            SignInMode::Reauthenticate { endpoints, .. } => server_of(endpoints).map(|(s, _)| s),
            SignInMode::Add => None,
            // a variant a newer porter adds: no held server, so the person is asked for one
            _ => None,
        };
        held.or_else(|| self.fixed.clone())
    }

    /// Starts the flow at `server` and sends the person to its page.
    async fn begin(&mut self, server: EndpointUrl) -> Move<Mid> {
        match start(&self.io, &server).await {
            Ok(started) => {
                let login = started.login.clone();
                let polling = Polling {
                    server,
                    started,
                    attempt: 0,
                    waited: Duration::ZERO,
                    hiccups: 0,
                };
                Move::Next(
                    SignInStep::OpenBrowser { url: login },
                    Mid::Polling(polling),
                )
            }
            Err(fault) => Move::Fail(fault),
        }
    }

    /// One poll: waits its turn, asks, and says what came of it.
    async fn poll(&mut self, mut polling: Polling) -> Move<Mid> {
        let pacing = self.io.pacing;
        if polling.waited >= pacing.limit {
            return Move::Fail(SignInFault::TimedOut);
        }
        let wait = pacing.wait(polling.attempt);
        self.io.sleep.sleep(wait).await;
        polling.attempt += 1;
        polling.waited += wait;
        match poll(&self.io, &polling.started).await {
            Err(fault) => Move::Fail(fault),
            Ok(Polled::Waiting) => {
                polling.hiccups = 0;
                Move::Next(SignInStep::Waiting, Mid::Polling(polling))
            }
            Ok(Polled::Hiccup) if polling.hiccups + 1 < MAX_HICCUPS => {
                polling.hiccups += 1;
                Move::Next(SignInStep::Waiting, Mid::Polling(polling))
            }
            Ok(Polled::Hiccup) => Move::Fail(SignInFault::Unreachable),
            Ok(Polled::Failed) => Move::Fail(SignInFault::Unreadable),
            Ok(Polled::Granted(granted)) => self.granted(polling.server, granted).await,
        }
    }

    /// The person approved: find out what the account can do, then review (a new account) or
    /// finish (a sign-in again, which only replaces the password).
    async fn granted(&mut self, dialled: EndpointUrl, granted: Granted) -> Move<Mid> {
        let server = granted.server.unwrap_or(dialled);
        let who = Who {
            server,
            login: granted.login,
            password: granted.password,
        };
        let found = match discover(&self.io, &who).await {
            Ok(found) => found,
            Err(fault) => return Move::Fail(fault),
        };
        let signed = signed(&who, found);
        match self.mode {
            SignInMode::Add => Move::Review(signed),
            SignInMode::Reauthenticate { .. } => Move::Done(signed),
            // a variant a newer porter adds: the sign-in ends as unreadable
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }
}

fn signed(who: &Who, found: Found) -> Signed {
    Signed::new(
        AccountLabel(format!("{}@{}", who.login.0, host_of(&who.server))),
        vec![(
            SecretPurpose::Password,
            Credential::Password(who.password.clone()),
        )],
        found.claims,
        found.endpoints,
        Restriction::none(),
    )
}

/// The host (and a port that is not the scheme's own) the account is shown by.
fn host_of(server: &EndpointUrl) -> String {
    let origin = server.origin();
    match origin.port == origin.scheme.default_port() {
        true => origin.host,
        false => format!("{}:{}", origin.host, origin.port),
    }
}

impl Flow for NextcloudFlow {
    type Mid = Mid;

    async fn start(&mut self) -> Move<Mid> {
        match self.known_server() {
            Some(server) => self.begin(server).await,
            None => Move::Next(
                SignInStep::AskFields(vec![plain(FieldKind::Server)]),
                Mid::AskedServer,
            ),
        }
    }

    async fn resume(&mut self, mid: Mid, input: SignInInput) -> Move<Mid> {
        match (mid, input) {
            (Mid::AskedServer, SignInInput::Fields(answers)) => {
                match text_of(&answers, FieldKind::Server)
                    .and_then(|t| EndpointUrl::typed_server(&t))
                {
                    Some(server) => self.begin(server).await,
                    None => Move::Fail(SignInFault::Unreadable),
                }
            }
            (Mid::Polling(polling), SignInInput::Poll) => self.poll(polling).await,
            _ => Move::Fail(SignInFault::Unreadable),
        }
    }
}

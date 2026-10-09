//! The Tailscale sign-in: ask Tailscale who is signed in, and
//!
//! - signed in: show the account (the review of a new one) and finish;
//! - signed out: ask Tailscale to start its own sign-in, hand the person its page to open, and
//!   wait until somebody is signed in;
//! - not installed, not running, not letting porter ask: say which, in the sheet's closed words.
//!
//! The page is Tailscale's (`https://login.tailscale.com/...`); porter opens nothing itself and
//! takes no password, key or code.

use porter_core::sheet::{SignInFault, SignInInput};
use porter_core::{AccountLabel, Restriction, WebUrl};
use porter_http::{SharedSleep, Sleep};
use porter_provider::{SignIn, SignInStep, Signed};
use porter_tailscale::{LocalApi, Standing, Status, TailscaleError};
use std::time::Duration;

/// How long between two looks at Tailscale while the person signs in, or while it settles.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How long the person has to finish signing in.
const SIGN_IN_LIMIT: Duration = Duration::from_secs(5 * 60);

/// How many looks Tailscale gets to leave "starting" or to give its page, a look each.
const SETTLE_LOOKS: u32 = 8;

#[derive(Debug)]
enum State {
    Fresh,
    /// The person is signing in on Tailscale's page.
    Waiting {
        waited: Duration,
    },
    /// Shown, waiting for the person's confirmation.
    Reviewing(Box<Signed>),
    Ended,
}

/// The Tailscale sign-in conversation.
#[derive(Debug)]
pub struct TailnetSignIn {
    api: LocalApi,
    sleep: SharedSleep,
    /// Whether a new account is shown for review first (adding), or the sign-in just ends
    /// (signing in again).
    review: bool,
    state: State,
}

/// The sheet's word for why Tailscale could not be asked.
fn fault_of(error: TailscaleError) -> SignInFault {
    match error {
        TailscaleError::NotInstalled => SignInFault::NotInstalled,
        TailscaleError::NotRunning | TailscaleError::TimedOut => SignInFault::NotRunning,
        TailscaleError::Refused => SignInFault::NotAllowed,
        TailscaleError::SignedOut => SignInFault::SignedOut,
        _ => SignInFault::Unreadable,
    }
}

impl TailnetSignIn {
    pub(super) fn new(api: LocalApi, sleep: SharedSleep, review: bool) -> Self {
        Self {
            api,
            sleep,
            review,
            state: State::Fresh,
        }
    }

    fn failed(&mut self, fault: SignInFault) -> SignInStep {
        self.state = State::Ended;
        SignInStep::Failed(fault)
    }

    /// Tailscale's status once it has left "starting" (a few looks at most), or why not.
    async fn settled(&self) -> Result<Status, TailscaleError> {
        for look in 0..SETTLE_LOOKS {
            let status = self.api.status().await?;
            if status.standing() != Standing::Changing {
                return Ok(status);
            }
            if look + 1 < SETTLE_LOOKS {
                self.sleep.sleep(LOOK_EVERY / 4).await;
            }
        }
        // Still starting after all that: nothing is answering for it.
        Err(TailscaleError::NotRunning)
    }

    fn signed(label: String) -> Signed {
        Signed {
            label: AccountLabel(label),
            credentials: Vec::new(),
            claims: Vec::new(),
            endpoints: Vec::new(),
            restriction: Restriction::none(),
        }
    }

    /// Somebody is signed in: review the account (adding) or finish (signing in again).
    fn signed_in(&mut self, status: &Status) -> SignInStep {
        let Some(label) = status.label() else {
            return self.failed(SignInFault::Unreadable);
        };
        let signed = Self::signed(label.clone());
        match self.review {
            true => {
                self.state = State::Reviewing(Box::new(signed));
                SignInStep::Review {
                    claims: Vec::new(),
                    endpoints: Vec::new(),
                    restriction: Restriction::none(),
                    label: AccountLabel(label),
                }
            }
            false => {
                self.state = State::Ended;
                SignInStep::Done(signed)
            }
        }
    }

    async fn start(&mut self) -> SignInStep {
        let status = match self.settled().await {
            Ok(status) => status,
            Err(error) => return self.failed(fault_of(error)),
        };
        match status.standing() {
            Standing::Ready => self.signed_in(&status),
            Standing::SignedOut => self.ask_for_the_page().await,
            Standing::Down | Standing::Changing => self.failed(SignInFault::NotRunning),
        }
    }

    /// Nobody is signed in: Tailscale is asked to start its own sign-in, and gives the page.
    async fn ask_for_the_page(&mut self) -> SignInStep {
        if let Err(error) = self.api.start_login().await {
            return self.failed(fault_of(error));
        }
        for look in 0..SETTLE_LOOKS {
            match self.api.status().await {
                Ok(status) => {
                    if let Some(url) = status
                        .auth_url
                        .as_deref()
                        .and_then(|u| WebUrl::parse(u).ok())
                    {
                        self.state = State::Waiting {
                            waited: Duration::ZERO,
                        };
                        return SignInStep::OpenBrowser { url };
                    }
                    if status.standing() == Standing::Ready {
                        return self.signed_in(&status);
                    }
                }
                Err(error) => return self.failed(fault_of(error)),
            }
            if look + 1 < SETTLE_LOOKS {
                self.sleep.sleep(LOOK_EVERY / 4).await;
            }
        }
        // No page came: the person opens the Tailscale app themselves.
        self.failed(SignInFault::SignedOut)
    }

    async fn poll(&mut self, waited: Duration) -> SignInStep {
        if waited >= SIGN_IN_LIMIT {
            return self.failed(SignInFault::TimedOut);
        }
        self.sleep.sleep(LOOK_EVERY).await;
        match self.api.status().await {
            Ok(status) if status.standing() == Standing::Ready => self.signed_in(&status),
            // Still signed out, or starting up after the sign-in: go on waiting.
            Ok(status) if matches!(status.standing(), Standing::SignedOut | Standing::Changing) => {
                self.state = State::Waiting {
                    waited: waited + LOOK_EVERY,
                };
                SignInStep::Waiting
            }
            Ok(_) => self.failed(SignInFault::NotRunning),
            Err(error) => self.failed(fault_of(error)),
        }
    }
}

impl SignIn for TailnetSignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let state = std::mem::replace(&mut self.state, State::Ended);
        match (state, input) {
            (_, SignInInput::Cancel) => SignInStep::Failed(SignInFault::Cancelled),
            (_, SignInInput::Start) => self.start().await,
            (State::Waiting { waited }, SignInInput::Poll) => self.poll(waited).await,
            (State::Reviewing(signed), SignInInput::Confirm(_)) => SignInStep::Done(*signed),
            _ => self.failed(SignInFault::Unreadable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_way_tailscale_cannot_be_asked_has_its_own_word() {
        let table = [
            (TailscaleError::NotInstalled, SignInFault::NotInstalled),
            (TailscaleError::NotRunning, SignInFault::NotRunning),
            (TailscaleError::TimedOut, SignInFault::NotRunning),
            (TailscaleError::Refused, SignInFault::NotAllowed),
            (TailscaleError::SignedOut, SignInFault::SignedOut),
            (TailscaleError::Malformed, SignInFault::Unreadable),
            (TailscaleError::NoSuchPeer, SignInFault::Unreadable),
        ];
        for (error, fault) in table {
            assert_eq!(fault_of(error), fault, "{error:?}");
        }
    }
}

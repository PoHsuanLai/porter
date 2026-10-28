//! The Tailscale sign-in flow: ask Tailscale who is signed in, and
//!
//! - signed in: show the account (the review of a new one) and finish;
//! - signed out: ask Tailscale to start its own sign-in, hand the person its page to open, and
//!   wait until somebody is signed in;
//! - not installed, not running, not letting porter ask: say which, in the sheet's closed words.
//!
//! The page is Tailscale's (`https://login.tailscale.com/...`); porter opens nothing itself and
//! takes no password, key or code.

use crate::conversation::{Flow, Move};
use porter_core::{AccountLabel, Restriction};
use porter_http::{SharedSleep, Sleep};
use porter_ids::WebUrl;
use porter_provider::{SignInStep, Signed};
use porter_sheet::{SignInFault, SignInInput};
use porter_tailscale::{LocalApi, Standing, Status, TailscaleError};
use std::time::Duration;

/// How long between two looks at Tailscale while the person signs in, or while it settles.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How long the person has to finish signing in.
const SIGN_IN_LIMIT: Duration = Duration::from_secs(5 * 60);

/// How many looks Tailscale gets to leave "starting" or to give its page, a look each.
const SETTLE_LOOKS: u32 = 8;

/// The Tailscale sign-in flow.
#[derive(Debug)]
pub struct TailnetFlow {
    api: LocalApi,
    sleep: SharedSleep,
    /// Whether a new account is shown for review first (adding), or the sign-in just ends
    /// (signing in again).
    review: bool,
}

/// The person is signing in on Tailscale's page.
#[derive(Debug)]
pub struct Waiting {
    waited: Duration,
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

impl TailnetFlow {
    pub(super) fn new(api: LocalApi, sleep: SharedSleep, review: bool) -> Self {
        Self { api, sleep, review }
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
        Signed::new(
            AccountLabel(label),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Restriction::none(),
        )
    }

    /// Somebody is signed in: review the account (adding) or finish (signing in again).
    fn signed_in(&self, status: &Status) -> Move<Waiting> {
        let Some(label) = status.label() else {
            return Move::Fail(SignInFault::Unreadable);
        };
        let signed = Self::signed(label.clone());
        match self.review {
            true => Move::Review(signed),
            false => Move::Done(signed),
        }
    }

    /// Nobody is signed in: Tailscale is asked to start its own sign-in, and gives the page.
    async fn ask_for_the_page(&self) -> Move<Waiting> {
        if let Err(error) = self.api.start_login().await {
            return Move::Fail(fault_of(error));
        }
        for look in 0..SETTLE_LOOKS {
            match self.api.status().await {
                Ok(status) => {
                    if let Some(url) = status
                        .auth_url
                        .as_deref()
                        .and_then(|u| WebUrl::parse(u).ok())
                    {
                        let waiting = Waiting {
                            waited: Duration::ZERO,
                        };
                        return Move::Next(SignInStep::OpenBrowser { url }, waiting);
                    }
                    if status.standing() == Standing::Ready {
                        return self.signed_in(&status);
                    }
                }
                Err(error) => return Move::Fail(fault_of(error)),
            }
            if look + 1 < SETTLE_LOOKS {
                self.sleep.sleep(LOOK_EVERY / 4).await;
            }
        }
        // No page came: the person opens the Tailscale app themselves.
        Move::Fail(SignInFault::SignedOut)
    }

    async fn poll(&self, waited: Duration) -> Move<Waiting> {
        if waited >= SIGN_IN_LIMIT {
            return Move::Fail(SignInFault::TimedOut);
        }
        self.sleep.sleep(LOOK_EVERY).await;
        match self.api.status().await {
            Ok(status) if status.standing() == Standing::Ready => self.signed_in(&status),
            // Still signed out, or starting up after the sign-in: go on waiting.
            Ok(status) if matches!(status.standing(), Standing::SignedOut | Standing::Changing) => {
                let waiting = Waiting {
                    waited: waited + LOOK_EVERY,
                };
                Move::Next(SignInStep::Waiting, waiting)
            }
            Ok(_) => Move::Fail(SignInFault::NotRunning),
            Err(error) => Move::Fail(fault_of(error)),
        }
    }
}

impl Flow for TailnetFlow {
    type Mid = Waiting;

    async fn start(&mut self) -> Move<Waiting> {
        let status = match self.settled().await {
            Ok(status) => status,
            Err(error) => return Move::Fail(fault_of(error)),
        };
        match status.standing() {
            Standing::Ready => self.signed_in(&status),
            Standing::SignedOut => self.ask_for_the_page().await,
            Standing::Down | Standing::Changing => Move::Fail(SignInFault::NotRunning),
            // A standing this build does not know is not one to sign in on.
            _ => Move::Fail(SignInFault::NotRunning),
        }
    }

    async fn resume(&mut self, waiting: Waiting, input: SignInInput) -> Move<Waiting> {
        match input {
            SignInInput::Poll => self.poll(waiting.waited).await,
            _ => Move::Fail(SignInFault::Unreadable),
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

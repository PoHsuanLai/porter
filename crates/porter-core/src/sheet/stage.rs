//! The sheet's state machine: `(Sheet, SheetEvent) -> (Sheet, effects)`. Pure; the host carries
//! the effects out (draw a view, call the provider's sign-in, store the account) and reports
//! what came back as the next event. The rules (look only when asked, add only when told to,
//! a password goes to the sign-in and nowhere else) are the ones of mailo's add-account
//! machine, which this one replaces.

use super::input::SheetInput;
use super::progress::{Progress, Review, ServiceChoice, SignInFault, SignInInput, UserCode};
use super::view::{FieldProblem, ProviderRow, ReviewView, SheetView, SignInView};
use crate::app_id::AppId;
use crate::endpoint::EndpointUrl;
use crate::id::{AccountId, ProviderId};
use crate::wire::ProviderHint;

/// What the sheet was opened for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A new account.
    Add {
        /// A provider to preselect.
        hint: ProviderHint,
        /// The app whose chooser found no fitting account: the last step then reads "Add, and
        /// allow it" and stores one grant, not a second prompt.
        allow: Option<AppId>,
    },
    /// Sign an existing account in again.
    Reauthenticate {
        /// The account.
        account: AccountId,
        /// Its provider.
        provider: ProviderId,
    },
}

/// Where the sheet stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// The provider list; nothing has been asked of anyone.
    Choosing(Vec<ProviderRow>),
    /// The form, with a field to mark after a refusal.
    Asking {
        /// The provider.
        provider: ProviderId,
        /// The fields.
        fields: Vec<super::fields::FieldSpec>,
        /// What to mark.
        problem: Option<FieldProblem>,
    },
    /// Something is running (a lookup, the sign-in, the store); a second press must not start
    /// it again.
    Working(ProviderId),
    /// Waiting for the person in the browser.
    Browser {
        /// The provider.
        provider: ProviderId,
        /// The page that was opened.
        url: EndpointUrl,
    },
    /// Waiting for the person to type a code at a page.
    Code {
        /// The provider.
        provider: ProviderId,
        /// The code.
        user_code: UserCode,
        /// The page.
        url: EndpointUrl,
    },
    /// Confirmed: the sign-in is finishing, and the account is stored with these choices only
    /// when it says it is done.
    Confirming {
        /// The provider.
        provider: ProviderId,
        /// The person's service choices.
        choices: Vec<ServiceChoice>,
    },
    /// What was found, shown and not used yet.
    Reviewing {
        /// The provider.
        provider: ProviderId,
        /// What was found.
        review: Review,
    },
    /// The account is stored.
    Added,
    /// It ended without an account; the person may retry or go back.
    Failed {
        /// The provider.
        provider: ProviderId,
        /// Why.
        fault: SignInFault,
    },
}

/// One sheet: what it is for and where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    /// What it was opened for.
    pub purpose: Purpose,
    /// Where it stands.
    pub stage: Stage,
    /// The providers the person may pick from; `Back` returns to this list.
    pub providers: Vec<ProviderRow>,
}

/// What happened to the sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetEvent {
    /// The person did something.
    Input(SheetInput),
    /// The sign-in said something.
    SignIn(Progress),
    /// The host stored the account.
    Stored,
    /// The host could not store it.
    StoreFailed,
    /// The sheet's owner left the bus.
    Left,
}

/// How the sheet ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetEnd {
    /// An account was added (or signed in again).
    Added,
    /// The person closed it.
    Dismissed,
    /// It failed.
    Failed(SignInFault),
}

/// What the host must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetEffect {
    /// Draw this.
    Show(SheetView),
    /// Tell the provider's sign-in this.
    Feed(SignInInput),
    /// Store the account the sign-in produced, with these service choices.
    Store(Vec<ServiceChoice>),
    /// Close the sheet.
    Close(SheetEnd),
}

impl Sheet {
    /// The sheet as first shown: the provider list, or, where the provider is already known (a
    /// hint, a re-sign-in), working on it until the sign-in says what it needs.
    pub fn new(purpose: Purpose, providers: Vec<ProviderRow>) -> Self {
        let stage = match &purpose {
            Purpose::Add {
                hint: ProviderHint::Provider(id),
                ..
            }
            | Purpose::Reauthenticate { provider: id, .. } => Stage::Working(id.clone()),
            Purpose::Add {
                hint: ProviderHint::Any,
                ..
            } => Stage::Choosing(providers.clone()),
        };
        Self {
            purpose,
            stage,
            providers,
        }
    }

    /// What the host draws for this state.
    pub fn view(&self) -> SheetView {
        let allow = match &self.purpose {
            Purpose::Add { allow, .. } => allow.clone(),
            Purpose::Reauthenticate { .. } => None,
        };
        match &self.stage {
            Stage::Choosing(rows) => SheetView::Providers(rows.clone()),
            Stage::Asking {
                provider,
                fields,
                problem,
            } => SheetView::SignIn(SignInView {
                provider: provider.clone(),
                fields: fields.clone(),
                problem: *problem,
            }),
            Stage::Working(provider) | Stage::Confirming { provider, .. } => {
                SheetView::Working(provider.clone())
            }
            Stage::Browser { provider, url } => SheetView::BrowserWait {
                provider: provider.clone(),
                url: url.clone(),
            },
            Stage::Code {
                provider,
                user_code,
                url,
            } => SheetView::ShowCode {
                provider: provider.clone(),
                user_code: user_code.clone(),
                url: url.clone(),
            },
            Stage::Reviewing { provider, review } => SheetView::Review(ReviewView {
                provider: provider.clone(),
                review: review.clone(),
                allow,
            }),
            Stage::Added => SheetView::Done,
            Stage::Failed { provider, fault } => SheetView::Failed {
                provider: provider.clone(),
                fault: *fault,
            },
        }
    }
}

/// The sheet after `event`, and what the host must do about it.
pub fn step(sheet: Sheet, event: SheetEvent) -> (Sheet, Vec<SheetEffect>) {
    machine::step(sheet, event)
}

mod machine;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod tests;

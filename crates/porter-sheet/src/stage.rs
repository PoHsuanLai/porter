//! The sheet's state machine: `(Sheet, SheetEvent) -> (Sheet, effects)`. Pure; the host carries
//! the effects out (draw a view, call the provider's sign-in, store the account) and reports
//! what came back as the next event. The rules (look only when asked, add only when told to,
//! a password goes to the sign-in and nowhere else) are the ones of mailo's add-account
//! machine, which this one replaces.

use super::input::SheetInput;
use super::manual::{Manual, edit_form};
use super::progress::{Progress, Review, SignInFault, SignInInput, UserCode};
use super::view::{FieldProblem, ProviderRow, ReviewView, SheetView, SignInView};
use porter_capability::KindToggle;
use porter_core::AppId;
use porter_core::EndpointUrl;
use porter_core::ProviderHint;
use porter_core::{AccountId, AuthKind, ProviderId};
use porter_ids::WebUrl;
use porter_tailscale::Machine;

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
    /// Ask an existing account for an access its provider asks only when it is first used (the
    /// whole of a drive). Like signing in again, except that the person reviews what is asked
    /// before it is kept.
    Extend {
        /// The account.
        account: AccountId,
        /// Its provider.
        provider: ProviderId,
    },
    /// Edit an existing account's servers: the server form of its protocol, prefilled from
    /// `current`, tried before the host is asked to keep anything. Only for an account whose
    /// sign-in is a password (`servers_editable`); the host asks that before it opens the sheet.
    EditServers {
        /// The account.
        account: AccountId,
        /// Its provider.
        provider: ProviderId,
        /// How it signs in: whether the password is asked as an app password.
        auth: AuthKind,
        /// Its servers as they are.
        current: Manual,
        /// The name shown in From to prefill, when the form asks for one.
        name: Option<String>,
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
        url: WebUrl,
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
        choices: Vec<KindToggle>,
    },
    /// What was found, shown and not used yet.
    Reviewing {
        /// The provider.
        provider: ProviderId,
        /// What was found.
        review: Review,
    },
    /// The servers of an edit were sent and are being tried; the form is kept, to be asked again
    /// with a field marked if they do not pass.
    Checking {
        /// The provider.
        provider: ProviderId,
        /// The form as sent, its answers the prefills.
        fields: Vec<super::fields::FieldSpec>,
        /// The field a refused credential is about: the token when one was typed, else the
        /// password.
        credential: super::fields::FieldKind,
    },
    /// The account is stored (or, for an edit, handed back).
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
    /// The person's computers on their tailnet, as far as the host knows them: an AI server's
    /// address may be plain `http` to one of these (`server_address`). Empty when the host has
    /// no view of the tailnet.
    pub own: Vec<Machine>,
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
    /// An account was added, signed in again, or had its servers edited.
    Added,
    /// The person closed it.
    Dismissed,
    /// It failed.
    Failed(SignInFault),
}

/// What the host must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
// `Show` grew past clippy's threshold with `ProviderRow::mark_face`; boxing it would change a
// variant every host matches, for an effect built once per sheet step.
#[allow(clippy::large_enum_variant)]
pub enum SheetEffect {
    /// Draw this.
    Show(SheetView),
    /// Tell the provider's sign-in this.
    Feed(SignInInput),
    /// Store the account the sign-in produced, with these service choices.
    Store(Vec<KindToggle>),
    /// Open the browser page again (`OpenAgain`), through the portal path.
    OpenBrowser(WebUrl),
    /// Close the sheet.
    Close(SheetEnd),
}

impl Sheet {
    /// The sheet as first shown: the provider list, or, where the provider is already known (a
    /// hint, a re-sign-in), working on it until the sign-in says what it needs. An edit of an
    /// account's servers opens on its form, prefilled (`edit_form`).
    pub fn new(purpose: Purpose, providers: Vec<ProviderRow>) -> Self {
        let stage = match &purpose {
            Purpose::Add {
                hint: ProviderHint::Provider(id),
                ..
            }
            | Purpose::Reauthenticate { provider: id, .. }
            | Purpose::Extend { provider: id, .. } => Stage::Working(id.clone()),
            Purpose::Add {
                hint: ProviderHint::Any,
                ..
            } => Stage::Choosing(providers.clone()),
            Purpose::EditServers {
                provider,
                auth,
                current,
                name,
                ..
            } => Stage::Asking {
                provider: provider.clone(),
                fields: edit_form(*auth, current, name.as_deref()),
                problem: None,
            },
        };
        Self {
            purpose,
            stage,
            providers,
            own: Vec::new(),
        }
    }

    /// The same sheet that knows the person's computers on their tailnet, so that an AI server's
    /// address may be plain `http` to one of them.
    #[must_use]
    pub fn with_own_machines(mut self, own: Vec<Machine>) -> Self {
        self.own = own;
        self
    }

    /// The row of `provider` among the providers this sheet lists.
    pub fn row_of(&self, provider: &ProviderId) -> Option<ProviderRow> {
        self.providers.iter().find(|r| r.id == *provider).cloned()
    }

    /// What the host draws for this state.
    pub fn view(&self) -> SheetView {
        let row = |provider: &ProviderId| self.row_of(provider);
        let allow = match &self.purpose {
            Purpose::Add { allow, .. } => allow.clone(),
            Purpose::Reauthenticate { .. }
            | Purpose::Extend { .. }
            | Purpose::EditServers { .. } => None,
        };
        match &self.stage {
            Stage::Choosing(rows) => SheetView::Providers(rows.clone()),
            Stage::Asking {
                provider,
                fields,
                problem,
            } => SheetView::SignIn(SignInView {
                provider: provider.clone(),
                row: row(provider),
                fields: fields.clone(),
                problem: *problem,
            }),
            Stage::Working(provider)
            | Stage::Confirming { provider, .. }
            | Stage::Checking { provider, .. } => SheetView::Working {
                provider: provider.clone(),
                row: row(provider),
            },
            Stage::Browser { provider, url } => SheetView::BrowserWait {
                provider: provider.clone(),
                row: row(provider),
                url: url.clone(),
            },
            Stage::Code {
                provider,
                user_code,
                url,
            } => SheetView::ShowCode {
                provider: provider.clone(),
                row: row(provider),
                user_code: user_code.clone(),
                url: url.clone(),
            },
            Stage::Reviewing { provider, review } => SheetView::Review(ReviewView {
                provider: provider.clone(),
                row: row(provider),
                review: review.clone(),
                allow,
                allow_label: None,
            }),
            Stage::Added => SheetView::Done,
            Stage::Failed { provider, fault } => SheetView::Failed {
                provider: provider.clone(),
                row: row(provider),
                fault: *fault,
            },
        }
    }
}

/// The sheet after `event`, and what the host must do about it.
pub fn step(sheet: Sheet, event: SheetEvent) -> (Sheet, Vec<SheetEffect>) {
    machine::step(sheet, event)
}

#[cfg(test)]
mod edit_tests;
mod machine;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod manual_tests;
#[cfg(test)]
mod tests;

//! The app-facing API of the account service (design/31 §5.1). An app asks for a capability,
//! never a brand. [`Porter`] is the handle, over a transport: [`Porter::accounts`] signs an account
//! in; where the app hosts the service itself, [`Porter::grants`] finds an account by capability,
//! asks for consent and opens the relays a grant allows. The transports are `porter-client-dbus`
//! (accountd on the session bus) and `porter-client-inproc` (the service hosted in the app); this
//! crate names neither a bus nor a service.
//!
//! ```no_run
//! use porter_client::{ConsentOffer, Found, GrantsError, GrantsLink, Grants};
//! use porter_core::consent::Usage;
//! use porter_core::{Candidate, DataClass};
//! use porter_match::Need;
//! use porter_wire::ParentWindow;
//!
//! async fn photos_account<T: GrantsLink>(
//!     grants: &Grants<T>,
//!     need: &Need,
//!     window: &ParentWindow,
//! ) -> Result<Option<Candidate>, GrantsError> {
//!     Ok(match grants.find(need, DataClass::Photos, Usage::Interactive).await? {
//!         Found::One(candidate) => Some(candidate),
//!         Found::Several(list) => list.into_iter().next(), // quire's AccountPicker lets the person pick
//!         Found::NeedsConsent(offer) => Some(grants.request_grant(&offer, window).await?),
//!         Found::None(_why) => None, // EmptyState + "Add Account…"
//!     })
//! }
//! ```
//!
//! No feature is on by default, and the crate reaches no zbus and no service crate. Whoever uses
//! a credential holds it, so no credential crosses a bus. Over D-Bus an app asks accountd's
//! `Identities1` for "Continue as" ([`Identities`]) and for a permit to use the person's AI keys
//! ([`AiKeys`]). Inference is `inferd-client`'s.

mod accounts;
mod ai_accounts;
mod ai_keys;
mod authenticated;
mod changes;
mod error;
mod found;
mod grants;
mod identities;
mod link;
mod porter;
mod spaces;
mod tailnet;

pub use accounts::{AccountChange, AccountRef, AccountView, Accounts, AccountsError, NeedsReauth};
pub use ai_accounts::{AiAccounts, AiAccountsError};
pub use ai_keys::{AiKeys, AiKeysError};
pub use authenticated::AuthenticatedStream;
pub use changes::Changes;
pub use error::{LinkError, Malformed};
pub use found::{ConsentOffer, Found, NoAccount, found};
pub use grants::{Grants, GrantsError};
pub use identities::{Identities, IdentitiesError, Picked};
pub use link::{
    AccountWatch, AccountsLink, AiAccountsLink, AiKeysLink, GrantsLink, IdentitiesLink, SpacesLink,
    TailnetLink,
};
pub use porter::Porter;
pub use spaces::{Spaces, SpacesError};
pub use tailnet::Tailnet;

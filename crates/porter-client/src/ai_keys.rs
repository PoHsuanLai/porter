//! The person's AI keys, as an app asks for them (`org.quire.Accounts1.Keys` on accountd). An AI
//! key is accountd's, and inferd is the one that sends it to an AI company: an app never sees
//! one. What an app gets is the person's yes to use their keys, a permit ([`GrantId`]), after
//! accountd shows the consent sheet; an app that holds a permit already is answered with it and
//! no sheet is drawn.
//!
//! ```ignore
//! let keys = porter.ai_keys();
//! match keys.request_key(DataClass::Documents, Usage::Interactive, &window).await {
//!     Ok(permit) => { /* ask inferd for a session as usual; the permit is what lets it */ }
//!     Err(AiKeysError::Refused(Refusal::Dismissed)) => { /* the person closed the sheet */ }
//!     Err(AiKeysError::Refused(Refusal::NoFittingAccount)) => {
//!         // no key is added: send the person to add one (`Accounts::add`), then ask again
//!     }
//!     Err(other) => report(other),
//! }
//! ```
//!
//! Adding a key is `Accounts::add`, the same sheet as for any account. Settings lists and revokes
//! permits through accountd's settings rows, not here. The agent, the agent launcher, the
//! computer-use daemon, the terminal and the compositor are refused.

use crate::error::LinkError;
use crate::link::AiKeysLink;
use porter_core::consent::Usage;
use porter_core::{DataClass, GrantId};
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// Why a call to accountd's Keys failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AiKeysError {
    /// accountd refused for a reason the person or the machine gave: the person closed the sheet
    /// (`Dismissed`) or refused (`Denied`), no key fits (`NoFittingAccount`), a key must be signed
    /// in again (`NeedsReauth`), or the service cannot do it now (`Unavailable`).
    #[error("refused")]
    Refused(Refusal),
    /// The call did not complete.
    #[error(transparent)]
    Link(#[from] LinkError),
}

/// accountd's Keys interface, as an app uses it.
#[derive(Debug)]
pub struct AiKeys<T> {
    pub(crate) link: Arc<T>,
}

impl<T> Clone for AiKeys<T> {
    fn clone(&self) -> Self {
        Self {
            link: Arc::clone(&self.link),
        }
    }
}

impl<T: AiKeysLink> AiKeys<T> {
    /// Shows the person accountd's consent sheet for this app's use of one of their AI keys, for
    /// data of `class` used as `usage` says, over `window`. The permit it answers is the person's
    /// yes, within the scope they chose.
    pub async fn request_key(
        &self,
        class: DataClass,
        usage: Usage,
        window: &ParentWindow,
    ) -> Result<GrantId, AiKeysError> {
        self.link.request_key(class, usage, window).await
    }
}

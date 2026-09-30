//! The consent sheet's seam: accounts-ui draws it for accountd; tests script the answers.

use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::wire::ParentWindow;
use std::future::Future;

/// Shows the chooser and consent sheet and returns the user's answer.
pub trait Prompter: Send + Sync {
    /// Asks, attached to `window`.
    fn ask(
        &self,
        ask: ConsentAsk,
        window: &ParentWindow,
    ) -> impl Future<Output = ConsentAnswer> + Send;
}

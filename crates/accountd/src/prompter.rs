//! The consent sheet drawn by accounts-ui, reached over the session bus. Not built yet.

use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::wire::ParentWindow;
use porter_service::Prompter;

/// accounts-ui's chooser and consent sheet.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SheetPrompter;

impl Prompter for SheetPrompter {
    async fn ask(&self, _ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        todo!("ask accounts-ui to show the sheet attached to the window and await its answer")
    }
}

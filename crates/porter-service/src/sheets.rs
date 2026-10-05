//! The sheets' seam (porter PLAN G5): the host that draws for accountd. A consent is one call; a
//! conversation (add an account, sign in again, a device code) is a link accountd updates with
//! views and reads the person's inputs from. accountd serves it over `org.quire.AccountsSheet1`
//! (sill draws), an app hosting porter in process draws it itself, tests script it.

use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::ParentWindow;
use std::fmt;
use std::future::Future;

/// What a conversation opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetOpen {
    /// The window the sheet attaches to.
    pub window: ParentWindow,
    /// What it shows first.
    pub view: SheetView,
}

/// Why a conversation could not go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetFault {
    /// No sheet host is reachable (sill is not running, no app draws).
    Unavailable,
    /// The host closed the sheet or left.
    Closed,
}

impl fmt::Display for SheetFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SheetFault::Unavailable => "no sheet host reachable",
            SheetFault::Closed => "sheet closed",
        })
    }
}

impl std::error::Error for SheetFault {}

/// Shows sheets for accountd and returns what the person did.
pub trait Sheets: Send + Sync {
    /// The open conversation `conversation` returns.
    type Link: SheetLink;

    /// Shows the chooser and consent sheet and returns the user's answer.
    fn consent(
        &self,
        ask: ConsentAsk,
        window: &ParentWindow,
    ) -> impl Future<Output = ConsentAnswer> + Send;

    /// Opens a conversation: the sheet is shown with `open.view`, and the link carries the
    /// rest. Dropping the link closes the sheet.
    fn conversation(
        &self,
        open: SheetOpen,
    ) -> impl Future<Output = Result<Self::Link, SheetFault>> + Send;
}

/// One open conversation with a sheet.
pub trait SheetLink: Send {
    /// Replaces what the sheet shows.
    fn update(&mut self, view: SheetView) -> impl Future<Output = Result<(), SheetFault>> + Send;

    /// The person's next input. Only the sheet's owner's input counts; the host checks that.
    fn input(&mut self) -> impl Future<Output = Result<SheetInput, SheetFault>> + Send;
}

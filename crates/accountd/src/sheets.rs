//! The sheets drawn by the sheet host (sill) over `org.quire.AccountsSheet1`. Not built yet.

use porter_core::consent::{ConsentAnswer, ConsentAsk};
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::ParentWindow;
use porter_service::{SheetFault, SheetLink, SheetOpen, Sheets};

/// The sheet host reached over the session bus: `Open`, `Update` and `Close` out, the `Input`
/// signal in (only from the connection that owns the handle).
#[derive(Debug, Clone, Copy)]
pub(crate) struct BusSheets;

/// One open handle on the host.
#[derive(Debug)]
pub(crate) struct BusLink;

impl Sheets for BusSheets {
    type Link = BusLink;

    async fn consent(&self, _ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        todo!("open a handle with `SheetView::Consent`, await its `Input(Answer)`, close it")
    }

    async fn conversation(&self, _open: SheetOpen) -> Result<BusLink, SheetFault> {
        todo!("`AccountsSheet1.Open` with the view as JSON; the link reads `Input` signals")
    }
}

impl SheetLink for BusLink {
    async fn update(&mut self, _view: SheetView) -> Result<(), SheetFault> {
        todo!("`AccountsSheet1.Update` for this handle")
    }

    async fn input(&mut self) -> Result<SheetInput, SheetFault> {
        todo!("the next `Input` signal of this handle from the owner's connection, as JSON")
    }
}

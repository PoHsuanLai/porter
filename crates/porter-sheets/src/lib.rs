//! The bus side of porter's sheets. Three daemons show the person a sheet (accountd for an
//! identity, inferd for an AI key, syncd for "Continue as"); what they share lives here, so each
//! is thin wiring over it:
//!
//! - [`BusSheets`] draws a sheet in the sheet host (sill) over `org.quire.AccountsSheet1`, as
//!   porter-service's `Sheets` seam, and checks that the host is who it says (its caller role is
//!   `SheetHost`). [`BusLink`] is one open sheet.
//! - [`AppNames`] says what a person calls an app: the caller table's name, then its desktop
//!   entry, then the agent's provider label.
//! - [`RequestObjects`] serves the Request object of a sheet method, shaped as portals' are, at
//!   the request root of the [`ServiceRoot`] it is made for: the method returns the object's path
//!   at once, a task runs the call, and its `Response` goes to the caller alone. [`OpenSheets`]
//!   keeps an app to one open sheet of each kind.

mod app_names;
mod request;
mod sheets;

pub use app_names::AppNames;
pub use request::{OpenSheets, RequestObjects, SheetClaim};
pub use sheets::{BusLink, BusSheets};

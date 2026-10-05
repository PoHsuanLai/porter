//! accountd's bus front end as a library, so the objects it serves are tested on a private bus
//! without the daemon: `org.quire.Accounts1` over [`porter_service::AccountService`].
//!
//! - `Manager` (`Query`, `Availability`, `Choose`, `AddAccount`), `Grants` and `Tokens` at the
//!   accountd path; one `Account` object per account (`Reauthenticate`) at its own path.
//! - The caller is the connection's, never the request's: a [`Callers`] seam says which app a
//!   bus sender is, and a sender it does not know is refused `AccessDenied` by the bus's own
//!   error name (a client reports that as `TransportError::Denied`).
//! - A refusal is the error `org.quire.Accounts1.Error.<Refusal>` (`errors`), as the in-process
//!   carrier's `AccountsReply::Refused`.
//! - The three sheet methods return a Request object at once (`request`): the answer is its
//!   `Response` signal, sent to the caller alone, and the object goes when it has been sent. A
//!   `Close` from the caller, or the caller leaving the bus, ends the sheet and no `Response`
//!   follows.
//!
//! Not served yet: the `Account` properties (`Id`, `Provider`, `Label`, `State`,
//! `Capabilities`), the manager's signals, `Adopt`, `Tokens.OpenAuthenticated`, `Peer`, the
//! Settings module and a caller table over `porter_dbus::Callers`. The binary still exits "not
//! implemented": its secret store and sheets are stubs and its families have no bodies.

mod account;
mod callers;
mod core;
mod errors;
mod grants;
mod manager;
mod request;

pub use callers::{Callers, TableCallers};
pub use core::{Host, serve};
pub use errors::RefusedError;

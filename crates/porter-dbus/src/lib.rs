//! porter's D-Bus API (design/31 §4.4): the buses of accountd (identities, Spaces, Tailnet),
//! inferd (inference, agents, the agent launcher, AI keys) and syncd, portal-shaped (a method
//! that shows UI returns a Request object whose `Response` signal carries the answer). Whoever
//! uses a credential holds it, so no bus carries a token or a grant: the app that wants an
//! identity asks `Identities1`, the one that wants an AI key asks inferd's `Keys`.
//!
//! Each interface is written once, as the trait its callers use, under `#[bus_interface]`
//! (`porter-dbus-macros`). The same trait gives the caller's `<Name>Proxy`, the `<Name>Server`
//! trait a daemon implements and the `<Name>Served<S>` object that serves an `S: <Name>Server`;
//! a daemon registers `<Name>Served(its_object)` and emits the interface's signals through it.
//! The introspection is the interface's own (`<Name>Served::<Unserved>::introspect`), and the
//! checked-in `dbus/*.xml` must equal it (see `tests/introspection.rs`). `sheet` and `pending`
//! are the portal shape of a sheet's answer, shared by both sides: the request path, the
//! response codes and results, and the caller's listener. The Request interface is one shape for
//! the three services that show sheets; [`ServiceRoot`] says which service's bus name, request
//! path root and error prefix a call is under. [`serve_ready`] is the daemons' start: the wait
//! between registering their objects and claiming their name.
//!
//! The interfaces are grouped by the daemon that owns them, each group a module with its bus
//! names, keys and row codecs, named through it:
//! - [`accounts`]: accountd's (`Manager`, `Account`, `Request`, `AccountsSheet`, `Identities`,
//!   `Spaces`, `Tailnet`);
//! - [`inference`]: inferd's (`Inference`, `Agents`, `Launcher`, `Keys`);
//! - [`sync`]: syncd's (`Sync`, `Storage`, and the Photos Picker with its feature).
//!
//! What every group shares is at the root: [`Details`], the call a server method answers, the
//! failure of a bus call, [`BusTarget`] and [`serve_ready`]. The portal
//! shape of a sheet's answer is [`accounts::sheet`], and who is calling is [`callers`].

mod args;
pub mod callers;
#[cfg(feature = "callers-file")]
mod callers_file;
mod failure;
mod json_value;
mod ready;
mod serve;
mod target;

pub mod accounts;
pub mod inference;
pub mod sync;

pub use args::Details;
pub use failure::{BusFailure, classify, is_invalid_args, is_limits_exceeded};
pub use json_value::{from_vardict, to_vardict};
pub use ready::serve_ready;
pub use serve::{Call, Document, Unserved};
pub use target::BusTarget;
/// The session-bus connection transports and daemons hold.
pub use zbus::Connection as BusConnection;
/// The bus library's error, which [`classify`] reads.
pub use zbus::Error as BusError;
/// The stream trait the proxies' signal streams implement.
pub use zbus::export::futures_core::Stream as BusStream;
/// The value types of a vardict, so a caller builds `Details` without its own zbus edge.
pub use zbus::zvariant;

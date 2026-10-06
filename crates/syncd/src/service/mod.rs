//! `org.quire.Sync1` at `/org/quire/Sync1`, served over the [`Hub`] (design/31 §4.4).
//!
//! - The caller is the connection's, found by porter-dbus's `Callers` (cgroup, the one table both
//!   daemons share); a sender nothing names is `AccessDenied` and a caller never says who it is.
//! - A dataset is named `<account>/<dataset>` (`Hub`). A caller sees the datasets it owns;
//!   Settings and the porter daemons see all. A name that is unknown, or not the caller's, answers
//!   the refusal `NoFittingAccount` (`org.quire.Accounts1.Error.NoFittingAccount`): nothing is
//!   there for this caller, and no dataset (the state before W6e/W6f) is the same answer, never a
//!   panic. A malformed name is `InvalidArgs`.
//! - `Status` is `anchor_age` (seconds, once there is an anchor), `pending`, `conflicts`,
//!   `paused` and, where the replica reports one, `quota` (`STATUS_KEY_QUOTA`: `used` and
//!   `total`, both `t`).
//! - `Resolve(dataset, conflict, how)` settles a stored conflict (`how` is `keep_local` or
//!   `keep_remote`; the `Conflict` signal's `number` is the conflict). Only the dataset's owning
//!   app may call it (Settings and the porter daemons see a dataset but do not own it: `Denied`);
//!   a dataset the caller cannot see answers `NoFittingAccount` as for the other methods, a
//!   conflict unknown or already settled `org.quire.Sync1.Error.NoSuchConflict`, any other `how`
//!   `InvalidArgs`. The dataset's driver settles it between cycles and runs the next soon.
//! - `Progress` and `Conflict` go to the connections that called and may see the dataset, never
//!   broadcast; a connection that leaves the bus is forgotten.

mod errors;
mod hub;
mod object;
mod resolve;
mod status;

pub use errors::RefusedError;
pub use hub::{Access, DatasetName, Event, Handle, Hub, Nudge, StatusSnapshot};
pub use object::serve;
pub use resolve::{ConflictNumber, How, Settle, SettleError, UnknownHow};
pub use status::{conflict_details, progress_details, status_details};

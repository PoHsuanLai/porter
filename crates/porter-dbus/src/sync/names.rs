//! syncd's bus name and objects, the option and result keys its methods take and give, and the
//! error-name prefixes its refusals use.

/// syncd's bus name.
pub const SYNC_BUS: &str = "org.quire.Sync1";
/// syncd's object.
pub const SYNC_PATH: &str = "/org/quire/Sync1";
/// Where syncd serves `org.quire.Storage1` (the drives in Files), on its own bus name.
pub const STORAGE_PATH: &str = "/org/quire/Storage1";
/// The key of the quota in `Sync1.Status` (a vardict `a{sv}` holding `used` as a `t` and, when
/// the provider reports a limit, `total` as a `t`). syncd builds it from porter-sync's `Quota`.
pub const STATUS_KEY_QUOTA: &str = "quota";
/// The key of the conflict's number in the `Sync1.Conflict` signal's details (an `x`): the
/// number `Sync1.Resolve` takes as `conflict`.
pub const CONFLICT_KEY_NUMBER: &str = "number";
/// `Sync1.Resolve`'s `how`: upload the local content over the replica's current version.
pub const RESOLVE_KEEP_LOCAL: &str = "keep_local";
/// `Sync1.Resolve`'s `how`: take the replica's version and drop the local change.
pub const RESOLVE_KEEP_REMOTE: &str = "keep_remote";
/// The prefix of the errors `Sync1` names itself (a refusal of accountd's vocabulary keeps its
/// `org.quire.Accounts1.Error.` name).
pub const SYNC_ERROR_PREFIX: &str = "org.quire.Sync1.Error.";
/// `Sync1.Resolve` on a conflict that is not there: never stored, or already settled
/// (`org.quire.Sync1.Error.NoSuchConflict`).
pub const SYNC_ERROR_NO_SUCH_CONFLICT: &str = "org.quire.Sync1.Error.NoSuchConflict";
/// `Sync1.ConfirmDiscard` on a dataset that is not held for confirmation: it never was, or the
/// discard was already confirmed (`org.quire.Sync1.Error.NothingHeld`).
pub const SYNC_ERROR_NOTHING_HELD: &str = "org.quire.Sync1.Error.NothingHeld";
/// `Sync1.SyncNow` on a dataset the person paused: it does not sync until it is resumed
/// (`org.quire.Sync1.Error.Paused`).
pub const SYNC_ERROR_PAUSED: &str = "org.quire.Sync1.Error.Paused";

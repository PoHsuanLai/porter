//! accountd's registry file (porter PLAN §2.3). The file store itself lives in the `porter-store`
//! library, so an app can keep its own registry the same way; accountd uses it from there and
//! resolves its directory once (the binary reads `$XDG_STATE_HOME/porter`; the store reads no
//! environment). `FileStore` stays exported here for one batch.

/// Where the file store lives now: `porter_store::FileStore`. Kept at this path for one batch.
#[doc(inline)]
pub use porter_store::FileStore;

/// The exit status of a daemon that refused the registry file (EX_CONFIG). `accountd.service`
/// names it in `RestartPreventExitStatus=`, so the unit stops instead of restarting.
pub const EXIT_REGISTRY_REFUSED: u8 = 78;

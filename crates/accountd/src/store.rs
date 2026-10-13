//! accountd's registry file (porter PLAN §2.3). The file store itself lives in the `porter-store`
//! library (`porter_store::FileStore`), so an app can keep its own registry the same way;
//! accountd resolves its directory once (the binary reads `$XDG_STATE_HOME/porter`; the store
//! reads no environment).

/// The exit status of a daemon that refused the registry file (EX_CONFIG). `accountd.service`
/// names it in `RestartPreventExitStatus=`, so the unit stops instead of restarting.
pub const EXIT_REGISTRY_REFUSED: u8 = 78;

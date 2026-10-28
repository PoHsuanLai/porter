//! The rig a jailed scenario drives porter with (test tooling: `publish = false`, never
//! installed, never in `dist/`; check-boundary lists it so nothing shipped depends on it).
//!
//! Three binaries, built on the modules here:
//!
//! - `porter-rig-servers`: starts `porter-fake-servers`' fakes on loopback, plants known secrets
//!   (`planted`), writes `rig.json` (`rigfile`) and serves the scenario's levers (`control`);
//!   `servers` is which flag starts which fake.
//! - `porter-rig-client`: acts as an app over the session bus (`client`); `fixture` makes the
//!   identity the daemons' `test-proc-root` builds read.
//! - `porter-rig-secrets`: a fake `org.freedesktop.secrets` (the `fake-secret-service` crate), in
//!   memory, plain algorithm, so the real accountd's `Oo7Secrets` works in a jail with no keyring.
//!
//! `porter-rig-storage`, the fake `org.quire.Storage1` for building Files, lives with syncd's
//! fakes in the `syncd-fakes` crate, under the same name and flags.

pub mod client;
pub mod control;
pub mod fixture;
pub mod planted;
pub mod rigfile;
pub mod servers;

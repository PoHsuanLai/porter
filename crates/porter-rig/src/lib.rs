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
//! - `porter-rig-secrets`: a fake `org.freedesktop.secrets` (`secrets_service`), in memory,
//!   plain algorithm, so the real accountd's `Oo7Secrets` works in a jail with no keyring.

pub mod client;
pub mod control;
pub mod fixture;
pub mod planted;
pub mod rigfile;
pub mod secrets_service;
pub mod servers;

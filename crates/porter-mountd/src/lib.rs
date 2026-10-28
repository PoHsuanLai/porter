//! porter-mountd: the one step syncd's sandbox refuses, done for it.
//!
//! syncd runs with `NoNewPrivileges=yes` and a mount namespace of its own, so it can neither
//! run the setuid `fusermount3` nor make a mount anyone else would see. porter-mountd is a small
//! user service, started by its socket (`porter-mountd.socket`), that does that one step: asked
//! to show a drive, it makes `~/Cloud/<name>`, runs `fusermount3` and hands the caller the open
//! FUSE connection; the caller serves the drive's files over it. It owns no state and reaches no
//! network, keyring or account data.
//!
//! - [`wire`]: the format both sides speak, the folder name check and the fd passing
//!   (SCM_RIGHTS).
//! - [`client`]: syncd's side ([`client::mount`], [`client::unmount`]).
//! - [`server`]: the helper's side ([`server::Helper`] over a [`server::Mounts`]; on a real
//!   computer [`server::Fusermount`]), and its socket ([`server::listener`]).
//! - [`mountinfo`]: what is mounted where.

pub mod client;
pub mod mountinfo;
pub mod server;
pub mod wire;

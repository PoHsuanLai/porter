//! A client of Tailscale's LocalAPI: what this computer's Tailscale says of itself, its
//! network and the computers on it, who an address on the network belongs to, and Tailscale's
//! own sign-in page. porter never links Tailscale's code and never reads what its program
//! prints: the one way in is the HTTP the program serves on its unix socket.
//!
//! The types and the readers of the answers are pure. [`LocalApi`], which makes the requests
//! over the socket, is behind the `io` feature. Field names are those of Tailscale v1.80.0
//! (BSD-3-Clause); each reader says which file it follows.

mod error;
mod notice;
mod object;
mod status;
mod time;
mod whois;

#[cfg(feature = "io")]
mod client;

pub use error::TailscaleError;
pub use notice::Notice;
pub use status::{Backend, Node, Standing, Status, Tailnet, User, UserId};
pub use whois::{WhoIs, WhoIsNode};

#[cfg(feature = "io")]
pub use client::{DEFAULT_SOCKET, LocalApi, Watch};

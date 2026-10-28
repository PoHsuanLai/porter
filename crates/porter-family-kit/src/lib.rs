//! What the protocol families are written with. A family writes a [`Flow`] (what it says and
//! asks, and what it does with the answer) and [`Conversation`] is its `SignIn`; the password
//! families (Nextcloud and the generic one) reach the world through an [`Io`], and share the
//! form fields, the Basic credential and the typed-password readers of [`password`].

pub mod conversation;
pub mod io;
pub mod password;

pub use conversation::{Conversation, Flow, Move};
pub use io::{Io, Pacing};

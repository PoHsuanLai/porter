//! The one protocol (design/31 §4.3): the requests an app makes of accountd and the replies, which
//! travel as D-Bus arguments (porter-dbus) and, for a session, as frames on the socket
//! `Inference1.Open` returns. Pure values and the framing of a stream of them; no I/O.

mod error;
mod frame;
mod reply;
mod request;

pub use error::FrameError;
pub use frame::{Envelope, MAX_FRAME, decode_frame, encode_frame};
pub use reply::{AccountsReply, Refusal};
pub use request::{AccountsRequest, ParentWindow};

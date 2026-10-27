//! The one protocol (design/31 §4.3): these values travel as D-Bus arguments (porter-dbus) and,
//! for a session, as frames on the socket `Inference1.Open` returns.

mod frame;
mod reply;
mod request;

pub use frame::{Envelope, FrameRead, MAX_FRAME, decode_frame, encode_frame};
pub use reply::{AccountsReply, Refusal};
pub use request::{AccountsRequest, ParentWindow, ProviderHint};

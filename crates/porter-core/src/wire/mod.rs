//! The one protocol with two carriers (design/31 §4.3): these values travel as D-Bus
//! arguments (porter-dbus) and as frames on the latchkey socket.

mod frame;
mod reply;
mod request;

pub use frame::{Envelope, FrameRead, MAX_FRAME, decode_frame, encode_frame};
pub use reply::{AccountsReply, Refusal};
pub use request::{AccountsRequest, LegacyItem, LegacyRef, ParentWindow, ProviderHint};

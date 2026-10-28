//! The app's client of inferd (design/31 §5.1): sessions, warm-ups, the places the assistant could
//! run, the person's other computers that lend or ask for models, and the models inferd runs.
//!
//! **This crate sits in porter until steward takes it.** It is what `porter-client` held of
//! inference, moved whole (`git mv`) when `porter-client` became the accounts client; the steward
//! lane lifts it out as it stands. It reaches accountd for nothing: the accounts, the AI keys and
//! the agent launcher are `porter-client`'s.
//!
//! An app asks for a capability, never a brand, and opens a session pinned to one model:
//! `Inference::over(DbusTransport::session().await?)` and then `session`, `prepare`, `infer`.
//! [`Transport`] is the seam; [`DbusTransport`] (feature `dbus`) reaches inferd on the session bus
//! and [`Brokered`] answers from a [`SessionHost`] the app runs itself.
//!
//! Features: none is on by default. `dbus` is the bus carrier (Linux), with the session `Open`
//! returns as frames over a Unix stream socket with descriptors riding on them, so it pulls in
//! the runtime. `engines` is [`engines::EngineHost`], a `SessionHost` that routes need -> engine
//! over a table the app gives and drives the OpenAI-compatible engines it points at (Ollama,
//! llama.cpp, LM Studio, vLLM, a company's API), with keys from a `KeySource` the app implements;
//! it brings stoker's HTTP client with TLS. `lending` is `TailnetLending`, which installs and
//! takes away the drop-in that lets the person's other computers reach inferd (the Settings
//! switch "Let my other computers use this computer's models"), through a `UnitManager` the
//! caller gives (`SessionUnits` with `dbus`). It never sets `ai.tailnet.serve`; the caller does,
//! after `enable` answers `Ok`.

mod computers;
#[cfg(feature = "engines")]
pub mod engines;
mod error;
#[cfg(feature = "dbus")]
mod guests;
mod inference;
#[cfg(feature = "lending")]
mod tailnet_lending;
mod transport;

pub use computers::{ComputerReach, NewComputer, NewComputerModel};
pub use error::{ClientError, ComputerReason, TransportError};
#[cfg(feature = "dbus")]
pub use guests::{GuestChange, GuestChanges};
pub use inference::Inference;
/// The client's side of one `Open` fd, from porter-infer.
pub use porter_infer::{
    ComputerName, InferSession, OpenOptions, PlaceId, SessionError, Traceparent,
};
/// What an [`engines::EngineHost`] is built from, from porter-infer.
#[cfg(feature = "engines")]
pub use porter_infer::{InferRefusal, Policy, Slot};
#[cfg(all(feature = "lending", feature = "dbus"))]
pub use tailnet_lending::SessionUnits;
#[cfg(feature = "lending")]
pub use tailnet_lending::{
    INFERD_UNIT, LendingConfig, LendingError, LendingState, SHIPPED_NAME, TailnetLending,
    UnitFailure, UnitManager,
};
pub use transport::{Brokered, InProcessSession, NoBroker, SessionHost, Transport};
#[cfg(feature = "dbus")]
pub use transport::{DbusSession, MAX_ATTACHMENTS};
#[cfg(feature = "dbus")]
pub use transport::{DbusTransport, Rows};

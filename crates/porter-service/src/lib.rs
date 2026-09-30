//! accountd's core (design/31 §4.1): the account registry, consent and the token broker over
//! four seams — [`porter_provider::Provider`] (protocol code), [`porter_secrets::Secrets`], a
//! [`Prompter`] that draws the consent sheet, and a [`Clock`]. Transport-free: accountd hosts
//! it behind D-Bus and the socket, and an app may host it in process.

mod choose;
mod clock;
mod prompter;
mod registry;
mod service;
mod token;

pub use clock::Clock;
pub use prompter::Prompter;
pub use registry::Registry;
pub use service::AccountService;
pub use token::secret_purpose;

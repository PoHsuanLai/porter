//! The AI keys inferd holds. The rule is that whoever uses a credential holds it: inferd is the
//! one that sends a key to an AI company, so the keys, the local runtimes, the agent logins and
//! the answers to "may this app use my AI keys" (the permits) live here, not in accountd.
//!
//! inferd hosts a porter `AccountService` in process over its own store, with only the AI and
//! agent families:
//!
//! - the registry is `keys/registry.json` under `$XDG_STATE_HOME/quire/inferd/`; the permits are
//!   the service's grants of the language-model kind, in the same file (there is no separate
//!   permit file);
//! - the keys are in the Secret Service under the namespace `porter.inferd` (in a `test-keys`
//!   build, `INFERD_KEYS=file:<absolute path>` names a plain file instead);
//! - the consent and add sheets are drawn by the shell through `org.quire.AccountsSheet1`
//!   (`porter-sheets`), asked for by [`KeysObject`] (`org.quire.Inference1.Keys`) and by Settings;
//! - [`LocalKeys`] is what the cloud turns ask: the permits an app holds and the key behind one,
//!   read in this process (no descriptor, no bus);
//! - [`AccountRows`] are the `accounts.<id>.*` rows of inferd's settings module, with the words
//!   accountd's rows had, so Settings reads them unchanged;
//! - [`news`] says when the accounts changed.
//!
//! Layer: this module is wiring over libraries (`porter-service`, `porter-families`,
//! `porter-store`, `porter-secrets`, `porter-sheets`); it adds only the seams inferd needs.

pub mod add;
mod audit;
mod build;
mod bus;
mod desk;
mod host;
mod local;
pub mod news;
mod rows;

pub use audit::KeyAudit;
pub use build::{
    Families, KeyService, KeysStartError, NAMESPACE, Opened, load_families, load_registry, open,
    served_ids,
};
pub use bus::{KeysError, KeysObject};
pub use desk::{KeyDesk, SecretsDesk, usable};
pub use host::KeyHost;
pub use local::LocalKeys;
pub use news::NewsStore;
pub use rows::{AccountRows, KeyRows, ServedSlot, SpendBook};

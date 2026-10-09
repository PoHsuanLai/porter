//! What Settings gives to add a computer of the person's own by hand (`Accounts::add_computer`):
//! the name they call it, and for each model it serves how to reach it from this computer.
//!
//! ```ignore
//! let place = accounts
//!     .add_computer(&NewComputer::new(
//!         "Studio".to_owned(),
//!         vec![NewComputerModel::new(ModelId::parse("qwen3-8b")?, ComputerReach::Port(8080))],
//!     ))
//!     .await?;                                    // PlaceId: computer:studio
//! ```

use porter_core::{ModelId, SecretText};
use std::path::PathBuf;

/// How a model on another computer is reached from this one: a connection already open on this
/// computer that leads to it, or a port on this computer that does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ComputerReach {
    /// The full path of the connection on this computer.
    Socket(PathBuf),
    /// The port on this computer.
    Port(u16),
}

/// One model a computer serves, and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewComputerModel {
    /// The assistant's catalogue id of the model.
    pub model: ModelId,
    /// How to reach it from this computer.
    pub reach: ComputerReach,
    /// The key it asks for, if any. inferd keeps it where only the owner can read it and never
    /// gives it back.
    pub key: Option<SecretText>,
}

impl NewComputerModel {
    /// A model reached by `reach`, with no key.
    pub fn new(model: ModelId, reach: ComputerReach) -> Self {
        Self {
            model,
            reach,
            key: None,
        }
    }

    /// The same model with the key it asks for.
    pub fn with_key(mut self, key: SecretText) -> Self {
        self.key = Some(key);
        self
    }
}

/// A computer of the person's own to add: what they call it, and the models it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NewComputer {
    /// What the person calls it (its place id is made from it).
    pub name: String,
    /// The models it serves.
    pub models: Vec<NewComputerModel>,
}

impl NewComputer {
    /// A computer called `name` serving `models`.
    pub fn new(name: String, models: Vec<NewComputerModel>) -> Self {
        Self { name, models }
    }
}

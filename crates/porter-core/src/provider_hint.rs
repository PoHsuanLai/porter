//! Which provider the add-account sheet opens on: a value of the request that asks for it
//! (`porter-wire`) and of the sheet's purpose (`porter-sheet`), so it lives below both.

use crate::ProviderId;
use serde::{Deserialize, Serialize};

/// Which provider the add sheet opens on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ProviderHint {
    /// The provider list.
    Any,
    /// This provider's form.
    Provider(ProviderId),
}

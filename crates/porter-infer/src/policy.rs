//! The user's AI policy: the local-only switch and a floor per data class (design/31 §5.5,
//! R14). Its values come from settings (`ai.local_only`, `ai.floor.<class>`); `proposed` holds
//! the design's defaults for them.

use porter_core::{DataClass, Locality};
use serde::{Deserialize, Serialize};

/// Whether cloud accounts exist at all for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOnly {
    /// Every cloud account is removed from every answer.
    On,
    /// Cloud accounts may serve classes whose floor allows it.
    Off,
}

/// How far a class's data may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Floor {
    /// This computer only.
    OnDevice,
    /// The user's own machines.
    LocalNetwork,
    /// Anywhere, cloud included.
    Anywhere,
}

impl Floor {
    /// Whether a model at `locality` may receive data with this floor.
    pub fn admits(self, locality: &Locality) -> bool {
        match locality {
            Locality::OnDevice => true,
            Locality::LocalNetwork => self >= Floor::LocalNetwork,
            Locality::Cloud { .. } => self == Floor::Anywhere,
        }
    }
}

/// One class's floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClassFloor {
    /// The class.
    pub class: DataClass,
    /// Its floor.
    pub floor: Floor,
}

/// The whole policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The switch.
    pub local_only: LocalOnly,
    /// Floors; a class without a row may go anywhere.
    pub floors: Vec<ClassFloor>,
}

impl Policy {
    /// The design's proposed defaults: local-only on; personal classes on this computer (voice
    /// and the person's prompts included, settings `ai.floor.voice` and `ai.floor.prompt`);
    /// `Public` and `AppOwn` anywhere.
    pub fn proposed() -> Self {
        let on_device = [
            DataClass::Mail,
            DataClass::Photos,
            DataClass::Notes,
            DataClass::Files,
            DataClass::Contacts,
            DataClass::Calendar,
            DataClass::Tasks,
            DataClass::Screen,
            DataClass::Clipboard,
            DataClass::Voice,
            DataClass::Prompt,
        ];
        Self {
            local_only: LocalOnly::On,
            floors: on_device
                .map(|class| ClassFloor {
                    class,
                    floor: Floor::OnDevice,
                })
                .to_vec(),
        }
    }

    /// The floor for `class`.
    pub fn floor(&self, class: DataClass) -> Floor {
        self.floors
            .iter()
            .find(|row| row.class == class)
            .map_or(Floor::Anywhere, |row| row.floor)
    }
}

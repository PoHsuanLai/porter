//! Facts about this machine that a person should know before more work is given to a model:
//! several helpers on one model, graphics memory nearly full, a long wait. Each is a fact, never
//! a refusal; the person may go ahead. Pure: the facts in, the notes out.

use porter_core::{Bytes, Count};
use serde::{Deserialize, Serialize};

/// A wait this long (requests ahead) is worth saying.
pub const QUEUE_LONG_FROM: Count = Count(3);

/// One fact about the machine, as the picker and the helper's card show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HardwareNote {
    /// Several helpers use this model at once, so replies will be slower.
    ModelShared {
        /// How many helpers use it now.
        helpers: Count,
    },
    /// The graphics memory left would not hold one more request on this model.
    GraphicsMemoryLow {
        /// What is free now.
        free: Bytes,
        /// About what one more request needs.
        one_more: Bytes,
    },
    /// Many requests are waiting for this model.
    QueueLong {
        /// How many are ahead of a new one.
        ahead: Count,
    },
}

/// What inferd knows about one model right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareFacts {
    /// How many helpers have requests on the model now.
    pub helpers: Count,
    /// How many requests are waiting for it.
    pub ahead: Count,
    /// Free graphics memory and what one more request needs, when the model runs on a graphics
    /// card and both are known (from the engine's own numbers once it has served, from the
    /// catalogue's size before that).
    pub memory: Option<MemoryFacts>,
}

impl Default for HardwareFacts {
    /// Nothing is using the model and nothing is known of the memory.
    fn default() -> Self {
        Self {
            helpers: Count(0),
            ahead: Count(0),
            memory: None,
        }
    }
}

/// Graphics memory now and what one more request would take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryFacts {
    /// Free now.
    pub free: Bytes,
    /// About what one more request needs.
    pub one_more: Bytes,
}

/// The notes these facts call for, most important first. None is a refusal.
pub fn hardware_notes(facts: &HardwareFacts) -> Vec<HardwareNote> {
    let shared = (facts.helpers.0 >= 2).then_some(HardwareNote::ModelShared {
        helpers: facts.helpers,
    });
    let low = facts
        .memory
        .filter(|memory| memory.free < memory.one_more)
        .map(|memory| HardwareNote::GraphicsMemoryLow {
            free: memory.free,
            one_more: memory.one_more,
        });
    let long =
        (facts.ahead >= QUEUE_LONG_FROM).then_some(HardwareNote::QueueLong { ahead: facts.ahead });
    [low, shared, long].into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(free: u64, one_more: u64) -> Option<MemoryFacts> {
        Some(MemoryFacts {
            free: Bytes(free),
            one_more: Bytes(one_more),
        })
    }

    #[test]
    fn notes_are_facts_from_the_numbers_and_never_a_refusal() {
        let cases = [
            (HardwareFacts::default(), vec![]),
            (
                HardwareFacts {
                    helpers: Count(1),
                    ..HardwareFacts::default()
                },
                vec![],
            ),
            (
                HardwareFacts {
                    helpers: Count(2),
                    ..HardwareFacts::default()
                },
                vec![HardwareNote::ModelShared { helpers: Count(2) }],
            ),
            (
                HardwareFacts {
                    memory: memory(100, 100),
                    ..HardwareFacts::default()
                },
                vec![],
            ),
            (
                HardwareFacts {
                    memory: memory(99, 100),
                    ..HardwareFacts::default()
                },
                vec![HardwareNote::GraphicsMemoryLow {
                    free: Bytes(99),
                    one_more: Bytes(100),
                }],
            ),
            (
                HardwareFacts {
                    ahead: Count(2),
                    ..HardwareFacts::default()
                },
                vec![],
            ),
            (
                HardwareFacts {
                    helpers: Count(4),
                    ahead: Count(3),
                    memory: memory(1, 2),
                },
                vec![
                    HardwareNote::GraphicsMemoryLow {
                        free: Bytes(1),
                        one_more: Bytes(2),
                    },
                    HardwareNote::ModelShared { helpers: Count(4) },
                    HardwareNote::QueueLong { ahead: Count(3) },
                ],
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(hardware_notes(&facts), expected, "{facts:?}");
        }
    }

    #[test]
    fn a_note_is_written_by_its_kind() {
        let json =
            serde_json::to_string(&HardwareNote::ModelShared { helpers: Count(2) }).expect("json");
        assert_eq!(json, r#"{"kind":"model_shared","v":{"helpers":2}}"#);
    }
}

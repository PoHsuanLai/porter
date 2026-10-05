//! How full a Storage account is, for Settings' "used / total" and for an upload that checks
//! before it sends (design/31 §2.2 `quota`).

use porter_core::Bytes;
use serde::{Deserialize, Serialize};

/// What a replica's account has used and may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Quota {
    /// Bytes in use.
    pub used: Bytes,
    /// Bytes allowed; absent when the provider reports no limit.
    pub total: Option<Bytes>,
}

impl Quota {
    /// The room left, or `None` when there is no limit. Zero when over it.
    pub fn free(&self) -> Option<Bytes> {
        self.total
            .map(|Bytes(total)| Bytes(total.saturating_sub(self.used.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_room_is_total_less_used_and_never_negative() {
        const CASES: &[(&str, u64, Option<u64>, Option<u64>)] = &[
            ("room", 10, Some(100), Some(90)),
            ("full", 100, Some(100), Some(0)),
            ("over", 120, Some(100), Some(0)),
            ("no limit", 10, None, None),
        ];
        for (name, used, total, free) in CASES {
            let quota = Quota {
                used: Bytes(*used),
                total: total.map(Bytes),
            };
            assert_eq!(quota.free(), free.map(Bytes), "{name}");
        }
    }

    #[test]
    fn a_quota_round_trips_and_pins_its_form() {
        let quota = Quota {
            used: Bytes(5),
            total: None,
        };
        let json = serde_json::to_string(&quota).expect("json");
        assert_eq!(json, r#"{"used":5,"total":null}"#);
        assert_eq!(serde_json::from_str::<Quota>(&json).expect("quota"), quota);
    }
}

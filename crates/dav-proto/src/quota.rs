//! A collection's quota, read from a parsed [`Multistatus`]: `quota-used-bytes` and
//! `quota-available-bytes` (RFC 4331). The request side is [`crate::propfind`] with
//! [`crate::names::QUOTA_USED`] and [`crate::names::QUOTA_AVAILABLE`].

use crate::Multistatus;
use crate::names::{QUOTA_AVAILABLE, QUOTA_USED};

/// A collection's quota; a field is `None` when the server did not give it (or gave no number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Quota {
    /// `quota-used-bytes`.
    pub used: Option<u64>,
    /// `quota-available-bytes`: what is left, `None` also for a server without a limit.
    pub available: Option<u64>,
}

/// The quota the first response reports.
pub fn quota(status: &Multistatus) -> Quota {
    let number = |name| status.first_found(name).and_then(|v| v.trim().parse().ok());
    Quota {
        used: number(QUOTA_USED),
        available: number(QUOTA_AVAILABLE),
    }
}

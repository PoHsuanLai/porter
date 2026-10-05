//! Where accountd's audit line goes: a seam, so the daemon appends `audit.jsonl`, an app
//! hosting porter appends its own, and tests collect entries. An entry never holds a value.

use porter_core::audit::AuditEntry;

/// Receives one entry per audited event. It cannot fail the call that caused it: a log that
/// cannot be written is the sink's to report, not the caller's.
pub trait AuditSink: Send + Sync {
    /// Records `entry`.
    fn record(&self, entry: AuditEntry);
}

/// Records nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAudit;

impl AuditSink for NoAudit {
    fn record(&self, _entry: AuditEntry) {}
}

//! The audit trail of finished turns: who, through what, how much; never the content
//! (`porter_infer::AuditEntry`). One [`SessionAudit`] per session (it knows the app); the entries
//! go to an [`AuditOut`]: nowhere, a JSON-lines file, or a test's memory.
//!
//! memoryd's `Record` is not the destination yet: it has no area tag for inference, and `prov`
//! no system part for inferd (see FINDINGS), and porter does not depend on almanac. The file is
//! the stand-in; its lines are `AuditEntry`'s serde form, so a bridge reads them unchanged.
//! `images` and `audio_ms` stay zero: the sink is told the reply, not the request.

use crate::clock::Clock;
use crate::serve::AuditSink;
use crate::session::SessionSpec;
use porter_core::{AppId, Bytes, Count, Tokens};
use porter_infer::{AuditEntry, InferReply, ServedBy, TokenUsage};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// Where audit entries go.
pub trait AuditOut: Send + Sync {
    /// Records one entry. A failure to write is the writer's to report; a turn never fails on it.
    fn append(&self, entry: &AuditEntry);
}

/// No memory on this desktop: entries are dropped.
#[derive(Debug, Clone, Copy, Default)]
pub struct Discard;

impl AuditOut for Discard {
    fn append(&self, _: &AuditEntry) {}
}

/// One JSON object per line, appended to a file that is created with its directory.
#[derive(Debug, Clone)]
pub struct JsonLines {
    path: PathBuf,
}

impl JsonLines {
    /// Appends to `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AuditOut for JsonLines {
    fn append(&self, entry: &AuditEntry) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
            writeln!(file, "{line}")
        };
        if let Err(why) = write() {
            // The daemon's one log path is standard error, prefixed with its name.
            eprintln!("inferd: audit: {}: {why}", self.path.display());
        }
    }
}

/// Entries kept in memory, for tests.
#[derive(Debug, Clone, Default)]
pub struct Memory(Arc<Mutex<Vec<AuditEntry>>>);

impl Memory {
    /// The entries so far.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl AuditOut for Memory {
    fn append(&self, entry: &AuditEntry) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(entry.clone());
    }
}

impl<T: AuditOut> AuditOut for Arc<T> {
    fn append(&self, entry: &AuditEntry) {
        (**self).append(entry);
    }
}

/// The tokens a reply spent (none for a refusal, a failure or a cancel).
pub fn usage_of(reply: &InferReply) -> TokenUsage {
    match reply {
        InferReply::Chat(chat) => chat.usage,
        InferReply::Embed(embed) => embed.usage,
        _ => TokenUsage {
            input: Tokens(0),
            output: Tokens(0),
            cached: Tokens(0),
        },
    }
}

/// The audit sink of one session.
#[derive(Debug, Clone)]
pub struct SessionAudit<O, C> {
    app: AppId,
    out: O,
    clock: C,
}

impl<O: AuditOut, C: Clock> SessionAudit<O, C> {
    /// Records this app's turns to `out`, stamped by `clock`.
    pub fn new(app: AppId, out: O, clock: C) -> Self {
        Self { app, out, clock }
    }

    fn entry(&self, served: &ServedBy, reply: &InferReply) -> AuditEntry {
        AuditEntry {
            at: self.clock.now(),
            app: self.app.clone(),
            account: served.account.clone(),
            model: served.model.clone(),
            locality: served.locality.clone(),
            usage: usage_of(reply),
            bytes_out: Bytes(0),
            images: Count(0),
            audio_ms: Count(0),
        }
    }
}

impl<O: AuditOut, C: Clock> AuditSink for SessionAudit<O, C> {
    fn record(&self, _spec: &SessionSpec, served: &ServedBy, reply: &InferReply) {
        self.out.append(&self.entry(served, reply));
    }
}

#[cfg(test)]
mod tests;

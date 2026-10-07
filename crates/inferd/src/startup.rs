//! Why an engine did not start, and what is known about it: the typed [`Cause`] a waiter is told,
//! the bounded tail of the engine's standard error, the check of an engine's socket path before
//! anything is spawned, and the line sink inferd's log goes through.
//!
//! A start that fails is a failure with a cause (the exit status, the last line the engine said,
//! the path that cannot be used), never a silent `NotReady` until a client gives up. Engine
//! standard error at startup holds no prompt or secret, but it is capped anyway: at most
//! [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes are kept, one line at most [`LINE_BYTES`] long,
//! with control characters (and ANSI colour sequences) taken out.

use engine_supervisor::{EngineId, ExitCode};
use model_catalog::MiB;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// The bytes of `sun_path` in `sockaddr_un` on Linux, the NUL included: a socket path of this many
/// bytes or more cannot be bound.
pub const SUN_PATH: usize = 108;

/// The most lines of an engine's standard error kept.
pub const TAIL_LINES: usize = 40;

/// The most bytes of an engine's standard error kept (the lines, not counting their breaks).
pub const TAIL_BYTES: usize = 8 * 1024;

/// The longest line kept; the rest of a longer one is dropped.
pub const LINE_BYTES: usize = 1024;

/// The longest tail a cause shows in its one line.
const SHORT_BYTES: usize = 200;

/// The last lines an engine wrote to standard error, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tail(pub Vec<String>);

impl Tail {
    /// The last line, cut to a short length: what a failure cause carries.
    pub fn short(&self) -> Option<String> {
        self.0.last().map(|line| cut(line, SHORT_BYTES))
    }
}

/// `text`, cut to at most `max` bytes at a character boundary.
fn cut(text: &str, max: usize) -> String {
    match text
        .char_indices()
        .find(|(at, ch)| at + ch.len_utf8() > max)
    {
        Some((at, _)) => text[..at].to_owned(),
        None => text.to_owned(),
    }
}

/// A line with its control characters and ANSI escape sequences taken out; a tab is a space.
fn clean(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() || next == '~' {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push(' '),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out.trim().to_owned()
}

/// The bounded buffer an engine's standard error is read into.
#[derive(Debug, Clone, Default)]
pub struct TailBuf {
    lines: VecDeque<String>,
    bytes: usize,
    partial: Vec<u8>,
}

impl TailBuf {
    /// Takes what the engine just wrote.
    pub fn push(&mut self, chunk: &[u8]) {
        for byte in chunk {
            if *byte == b'\n' {
                self.end_line();
            } else if self.partial.len() < LINE_BYTES {
                self.partial.push(*byte);
            }
        }
    }

    /// The engine's standard error ended: a last line with no break counts.
    pub fn finish(&mut self) {
        self.end_line();
    }

    fn end_line(&mut self) {
        let line = clean(&std::mem::take(&mut self.partial));
        if line.is_empty() {
            return;
        }
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > TAIL_LINES || self.bytes > TAIL_BYTES {
            match self.lines.pop_front() {
                Some(old) => self.bytes -= old.len(),
                None => break,
            }
        }
    }

    /// The lines kept, and the unfinished one when the engine has not ended it.
    pub fn tail(&self) -> Tail {
        let mut lines: Vec<String> = self.lines.iter().cloned().collect();
        let open = clean(&self.partial);
        if !open.is_empty() {
            lines.push(open);
        }
        Tail(lines)
    }
}

/// Why an engine is not ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The process ended before it answered.
    Exited {
        /// Its exit status (`-1` when a signal ended it).
        code: ExitCode,
        /// The last it said on standard error.
        tail: Tail,
    },
    /// The process ran and did not answer in time.
    NeverReady {
        /// The last it said on standard error.
        tail: Tail,
    },
    /// The socket path is too long to bind (`len` bytes; the limit is [`SUN_PATH`]).
    SocketPathTooLong {
        /// The path.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
    },
    /// Something that is not a socket is at the socket path; it is kept.
    SocketPathTaken {
        /// The path.
        path: PathBuf,
    },
    /// The socket path could not be looked at or a stale socket there could not be removed.
    SocketPathUnusable {
        /// The path.
        path: PathBuf,
        /// What the system said.
        kind: std::io::ErrorKind,
    },
    /// The program could not be run.
    CannotSpawn {
        /// The program.
        program: PathBuf,
        /// What the system said.
        kind: std::io::ErrorKind,
    },
    /// The host refused to start it and said no more.
    Refused,
    /// The GPU has no room for it.
    NoRoom {
        /// What it needs.
        need: MiB,
        /// What is free.
        free: MiB,
    },
    /// The model's profile cannot be run.
    BadProfile,
    /// Nothing is known (the supervisor is not running, or the engine is not one of its own).
    Unknown,
}

impl Cause {
    /// Whether a person cannot fix this by waiting: a bad path or a missing program stays so, and
    /// is logged at error.
    pub fn is_setup(&self) -> bool {
        matches!(
            self,
            Cause::SocketPathTooLong { .. }
                | Cause::SocketPathTaken { .. }
                | Cause::SocketPathUnusable { .. }
                | Cause::CannotSpawn { .. }
                | Cause::Refused
        )
    }

    /// The tail of standard error, when this cause has one.
    pub fn tail(&self) -> Option<&Tail> {
        match self {
            Cause::Exited { tail, .. } | Cause::NeverReady { tail } => Some(tail),
            _ => None,
        }
    }

    /// Whether a later request waits out a pause before the engine is tried again (a failure to
    /// start; not the GPU being full, which a request may find freed at once).
    pub fn pauses(&self) -> bool {
        !matches!(
            self,
            Cause::NoRoom { .. } | Cause::BadProfile | Cause::Unknown
        )
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said = |f: &mut fmt::Formatter<'_>, tail: &Tail| match tail.short() {
            Some(line) => write!(f, ": {line}"),
            None => Ok(()),
        };
        match self {
            Cause::Exited { code, tail } if code.0 < 0 => {
                f.write_str("the engine was killed before it was ready")?;
                said(f, tail)
            }
            Cause::Exited { code, tail } => {
                write!(
                    f,
                    "the engine exited with status {} before it was ready",
                    code.0
                )?;
                said(f, tail)
            }
            Cause::NeverReady { tail } => {
                f.write_str("the engine did not become ready in time")?;
                said(f, tail)
            }
            Cause::SocketPathTooLong { path, len } => write!(
                f,
                "the socket path {} is {len} bytes; a Unix socket path must be under {SUN_PATH} \
                 (sun_path, the NUL included)",
                path.display()
            ),
            Cause::SocketPathTaken { path } => write!(
                f,
                "{} exists and is not a socket; it is kept and the engine is not started",
                path.display()
            ),
            Cause::SocketPathUnusable { path, kind } => {
                write!(
                    f,
                    "the socket path {} cannot be used: {kind}",
                    path.display()
                )
            }
            Cause::CannotSpawn { program, kind } => {
                write!(f, "{} cannot be run: {kind}", program.display())
            }
            Cause::Refused => f.write_str("the host refused to start the engine"),
            Cause::NoRoom { need, free } => {
                write!(
                    f,
                    "no room on the GPU: needs {} MiB, {} MiB free",
                    need.0, free.0
                )
            }
            Cause::BadProfile => f.write_str("the model's engine profile cannot be run"),
            Cause::Unknown => f.write_str("the engine is unknown to the supervisor"),
        }
    }
}

/// Looks at `path` before an engine is spawned on it: a path too long to bind is refused, a
/// socket left by an earlier run is removed, anything else there is refused and kept (a symlink
/// is not a socket, and is not followed).
pub fn clear_socket(path: &Path) -> Result<(), Cause> {
    let len = path.as_os_str().as_bytes().len();
    if len >= SUN_PATH {
        return Err(Cause::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
        });
    }
    let unusable = |kind| Cause::SocketPathUnusable {
        path: path.to_path_buf(),
        kind,
    };
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            std::fs::remove_file(path).map_err(|e| unusable(e.kind()))
        }
        Ok(_) => Err(Cause::SocketPathTaken {
            path: path.to_path_buf(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(unusable(e.kind())),
    }
}

/// How serious a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Something went wrong that a later try may mend.
    Warn,
    /// Something is set up wrong.
    Error,
}

/// Where inferd's log lines go: standard error, or what a test puts in its place.
#[derive(Clone)]
pub struct Log(Arc<Sink>);

type Sink = dyn Fn(Level, &str) + Send + Sync;

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Log")
    }
}

impl Default for Log {
    fn default() -> Self {
        Self(Arc::new(|level, text| {
            let name = match level {
                Level::Warn => "warn",
                Level::Error => "error",
            };
            for line in text.lines() {
                eprintln!("inferd: {name}: {line}");
            }
        }))
    }
}

impl Log {
    /// A log that hands every line to `sink`.
    pub fn to(sink: impl Fn(Level, &str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(sink))
    }

    /// Writes one entry (it may span lines).
    pub fn emit(&self, level: Level, text: &str) {
        (self.0)(level, text);
    }

    /// The entry for a failed start of `engine`.
    pub fn failed(&self, engine: &EngineId, cause: &Cause) {
        let level = if cause.is_setup() {
            Level::Error
        } else {
            Level::Warn
        };
        let mut text = format!("engine {} did not start: {cause}", engine.0);
        if let Some(tail) = cause.tail().filter(|tail| !tail.0.is_empty()) {
            text.push_str("\nengine standard error, last lines:");
            for line in &tail.0 {
                text.push_str("\n  | ");
                text.push_str(line);
            }
        }
        self.emit(level, &text);
    }
}

/// What the host of the engines knows that the supervisor's machine does not: why a spawn was
/// refused, and what a process said on standard error. The host writes it, the driver reads it.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    notes: Arc<Mutex<BTreeMap<EngineId, Note>>>,
    log: Log,
}

#[derive(Debug, Default)]
struct Note {
    refusal: Option<Cause>,
    live: Arc<Mutex<TailBuf>>,
}

impl Diagnostics {
    /// The same, logging through `log`.
    pub fn logging_to(self, log: Log) -> Self {
        Self { log, ..self }
    }

    /// The log lines go through.
    pub fn log(&self) -> &Log {
        &self.log
    }

    fn notes(&self) -> std::sync::MutexGuard<'_, BTreeMap<EngineId, Note>> {
        self.notes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A start begins: what was known of the last one is forgotten; the buffer to read the new
    /// process's standard error into.
    pub fn begin(&self, id: &EngineId) -> Arc<Mutex<TailBuf>> {
        let note = Note::default();
        let live = Arc::clone(&note.live);
        self.notes().insert(id.clone(), note);
        live
    }

    /// The start was refused for this cause.
    pub fn refuse(&self, id: &EngineId, cause: Cause) {
        self.notes().entry(id.clone()).or_default().refusal = Some(cause);
    }

    /// Why the last spawn of `id` was refused, when it was.
    pub fn refusal(&self, id: &EngineId) -> Option<Cause> {
        self.notes().get(id).and_then(|note| note.refusal.clone())
    }

    /// What the last process of `id` said on standard error.
    pub fn tail(&self, id: &EngineId) -> Tail {
        let live = self.notes().get(id).map(|note| Arc::clone(&note.live));
        live.map(|live| live.lock().unwrap_or_else(PoisonError::into_inner).tail())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;

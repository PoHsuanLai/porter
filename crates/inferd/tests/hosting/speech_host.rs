//! A fake speech host: the `speech-host` protocol (`speech_provider::host_wire`, a 4-byte length
//! and JSON) served on a Unix socket, and an engine host that "runs" it when the supervisor
//! spawns the speech engine. It plays the part of the real binary and never loads a model or a
//! native library.
//!
//! What it says: for the n-th audio chunk of an utterance, a `Partial` of the first n words of
//! its script; on `End`, a `Final` of all of them and `Done`. What it records: every request it
//! was begun with, the chunks and samples it received, its `Cancel`s and its `Hello`s.
#![allow(dead_code)]

use engine_supervisor::{EngineHost, EngineId, ExitCode, HostError, UnitSpec};
use speech_provider::{
    AudioMs, HeardText, HostIn, HostOut, HostVocab, SampleIndex, SttEnd, SttRequest,
    TranscriptEvent, decode_frame, encode_frame, frame_length,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// What the host says.
#[derive(Debug, Clone)]
pub struct Words(pub Vec<&'static str>);

/// What it saw, shared with the test.
#[derive(Debug, Default)]
pub struct Seen {
    pub hellos: usize,
    pub requests: Vec<SttRequest>,
    pub chunks: usize,
    pub samples: u64,
    pub cancels: usize,
    pub ends: usize,
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn read_in(stream: &mut UnixStream) -> Option<HostIn> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await.ok()?;
    let mut body = vec![0_u8; frame_length(header).ok()?];
    stream.read_exact(&mut body).await.ok()?;
    decode_frame(&body).ok()
}

async fn write_out(stream: &mut UnixStream, out: &HostOut) -> bool {
    match encode_frame(out) {
        Ok(bytes) => stream.write_all(&bytes).await.is_ok(),
        Err(_) => false,
    }
}

struct Utterance {
    model: speech_provider::SttRequest,
    chunks: usize,
    samples: u64,
}

async fn serve_connection(mut stream: UnixStream, words: Words, seen: Arc<Mutex<Seen>>) {
    let mut utterance: Option<Utterance> = None;
    while let Some(message) = read_in(&mut stream).await {
        let replies: Vec<HostOut> = match message {
            HostIn::Hello { .. } => {
                lock(&seen).hellos += 1;
                vec![HostOut::Hello {
                    vocab: HostVocab::CURRENT,
                    models: Vec::new(),
                }]
            }
            HostIn::Begin(request) => {
                lock(&seen).requests.push(request.clone());
                utterance = Some(Utterance {
                    model: request,
                    chunks: 0,
                    samples: 0,
                });
                Vec::new()
            }
            HostIn::Audio(chunk) => {
                let mut state = lock(&seen);
                state.chunks += 1;
                state.samples += u64::from(chunk.samples());
                drop(state);
                match utterance.as_mut() {
                    Some(utterance) => {
                        utterance.chunks += 1;
                        utterance.samples += u64::from(chunk.samples());
                        let said = words
                            .0
                            .iter()
                            .take(utterance.chunks)
                            .copied()
                            .collect::<Vec<_>>();
                        if said.is_empty() {
                            Vec::new()
                        } else {
                            vec![HostOut::Event(TranscriptEvent::Partial {
                                text: HeardText(said.join(" ")),
                                from: SampleIndex(0),
                            })]
                        }
                    }
                    None => Vec::new(),
                }
            }
            HostIn::End => {
                lock(&seen).ends += 1;
                match utterance.take() {
                    Some(utterance) => {
                        let text = HeardText(words.0.join(" "));
                        let rate = u64::from(utterance.model.format.rate.0).max(1);
                        vec![
                            HostOut::Event(TranscriptEvent::Final {
                                text: text.clone(),
                                from: SampleIndex(0),
                                to: SampleIndex(utterance.samples),
                            }),
                            HostOut::Done(SttEnd {
                                text,
                                audio: AudioMs(
                                    u32::try_from(utterance.samples * 1_000 / rate)
                                        .unwrap_or(u32::MAX),
                                ),
                                served: utterance.model.model,
                            }),
                        ]
                    }
                    None => Vec::new(),
                }
            }
            HostIn::Cancel => {
                lock(&seen).cancels += 1;
                utterance = None;
                Vec::new()
            }
        };
        for out in replies {
            if !write_out(&mut stream, &out).await {
                return;
            }
        }
    }
}

/// Serves the protocol on `socket` until dropped. Connections are served one at a time, as the
/// real host does.
#[derive(Debug)]
pub struct FakeSpeechHost {
    pub seen: Arc<Mutex<Seen>>,
    socket: PathBuf,
    task: JoinHandle<()>,
}

impl FakeSpeechHost {
    pub fn start(socket: &Path, words: Words) -> Self {
        Self::start_into(socket, words, Arc::default())
    }

    /// As `start`, recording into `seen` (so several runs of one engine share a record).
    pub fn start_into(socket: &Path, words: Words, seen: Arc<Mutex<Seen>>) -> Self {
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket).expect("bind the host's socket");
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                serve_connection(stream, words.clone(), Arc::clone(&log)).await;
            }
        });
        Self {
            seen,
            socket: socket.to_path_buf(),
            task,
        }
    }

    /// Dies as a crashed process does: the listener is gone, the socket file stays behind.
    pub fn crash(&self) {
        self.task.abort();
    }
}

impl Drop for FakeSpeechHost {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

struct Running {
    host: FakeSpeechHost,
    exit: watch::Sender<Option<ExitCode>>,
}

/// An engine host that runs the fake speech host in place of the speech engine's program: it
/// takes the socket from the unit's `--socket` argument, as the real program does, and keeps
/// every unit it was asked to run. A unit of another program is recorded and starts nothing.
#[derive(Clone)]
pub struct SpeechEngines {
    words: Words,
    running: Arc<Mutex<BTreeMap<EngineId, Running>>>,
    exits: Arc<Mutex<BTreeMap<EngineId, watch::Receiver<Option<ExitCode>>>>>,
    pub units: Arc<Mutex<Vec<(EngineId, UnitSpec)>>>,
    pub seen: Arc<Mutex<Seen>>,
}

impl std::fmt::Debug for SpeechEngines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpeechEngines")
    }
}

impl SpeechEngines {
    pub fn new(words: &[&'static str]) -> Self {
        Self {
            words: Words(words.to_vec()),
            running: Arc::default(),
            exits: Arc::default(),
            units: Arc::default(),
            seen: Arc::default(),
        }
    }

    /// How many times the engine was spawned.
    pub fn spawns(&self) -> usize {
        lock(&self.units).len()
    }

    /// Kills the engine's process, as a crash: it exits with a signal's code.
    pub fn crash(&self, id: &EngineId) {
        if let Some(running) = lock(&self.running).remove(id) {
            running.host.crash();
            let _ = running.exit.send(Some(ExitCode(139)));
        }
    }
}

fn socket_of(unit: &UnitSpec) -> Option<PathBuf> {
    unit.args
        .windows(2)
        .find(|pair| pair[0].0 == "--socket")
        .map(|pair| PathBuf::from(&pair[1].0))
}

impl EngineHost for SpeechEngines {
    async fn spawn(&self, id: &EngineId, unit: &UnitSpec) -> Result<(), HostError> {
        lock(&self.units).push((id.clone(), unit.clone()));
        let socket = socket_of(unit).ok_or(HostError::Refused)?;
        // Every run reports into the one record the test reads.
        let host = FakeSpeechHost::start_into(&socket, self.words.clone(), Arc::clone(&self.seen));
        let (exit, exited) = watch::channel(None);
        lock(&self.exits).insert(id.clone(), exited);
        lock(&self.running).insert(id.clone(), Running { host, exit });
        Ok(())
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        match lock(&self.running).remove(id) {
            Some(running) => {
                let _ = running.exit.send(Some(ExitCode(0)));
                Ok(())
            }
            None => Err(HostError::NotRunning),
        }
    }

    async fn exited(&self, id: &EngineId) -> ExitCode {
        let Some(mut exit) = lock(&self.exits).get(id).cloned() else {
            return ExitCode(-1);
        };
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            if exit.changed().await.is_err() {
                return ExitCode(-1);
            }
        }
    }
}

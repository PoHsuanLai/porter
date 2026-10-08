//! A fake `org.quire.Inference1` for the client's bus tests. `Open` answers with one end of a
//! socketpair and serves the other end: it decodes `ClientFrame`s (collecting the memfds that
//! ride on them), plays them into a scripted `FakeInferSession` and writes its events back as
//! frames. How it misbehaves is a `Behaviour`, so the client's error paths run too.

use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_core::{DataClass, Need, Tier};
use porter_dbus::{Details, NeedArg, need_from_dbus};
use porter_fake::{FakeInferSession, Script};
use porter_infer::{ClientFrame, InferEvent, InferRefusal, InferReply, InferSession};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
use std::io::IoSliceMut;
use std::mem::MaybeUninit;
use std::os::unix::net::UnixStream as StdStream;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWriteExt, Interest};
use tokio::net::UnixStream;
use zbus::fdo;
use zbus::zvariant::OwnedFd;

/// What one `Open` call asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub need: Need,
    pub class: String,
    pub tier: String,
    pub traceparent: Option<String>,
    pub usage: Option<String>,
}

/// What the fake saw, shared with the test.
#[derive(Debug, Default)]
pub struct Seen {
    pub opens: Vec<Opened>,
    /// What each `Prepare` call asked for.
    pub prepares: Vec<Opened>,
    pub frames: Vec<ClientFrame>,
    /// The bytes of each descriptor received, in order.
    pub attachments: Vec<Vec<u8>>,
}

/// How the fake serves a session.
#[derive(Debug, Clone)]
pub enum Behaviour {
    /// Play these scripts, one per request kind.
    Scripted(Vec<Script>),
    /// Refuse at once: `Finished(Refused(..))` before any request.
    Refuse(InferRefusal),
    /// Close the socket after reading the first frame.
    HangUp,
    /// Write these bytes at once, whatever they are.
    Raw(Vec<u8>),
    /// Play these scripts, writing every frame a byte at a time.
    Dribble(Vec<Script>),
    /// The real session server over a fixed route and these scripts: `Routed`, `Waiting`,
    /// queueing and refusals come from the session machine, not from the script.
    Served(Served),
}

/// The seams of the real server for one test.
#[derive(Debug, Clone)]
pub struct Served {
    pub route: super::served::Route,
    pub gate: super::served::Gate,
    pub runner: super::served::Scripted,
}

/// The interface object.
#[derive(Debug)]
pub struct FakeInferd {
    pub seen: Arc<Mutex<Seen>>,
    behaviour: Behaviour,
    /// What `Prepare` answers (the slug).
    prepare: Arc<Mutex<String>>,
}

impl FakeInferd {
    pub fn new(behaviour: Behaviour) -> (Self, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        (
            Self {
                seen: Arc::clone(&seen),
                behaviour,
                prepare: Arc::new(Mutex::new("ready".to_owned())),
            },
            seen,
        )
    }

    /// The same fake, answering `Prepare` with `slug`.
    pub fn preparing(self, slug: &str) -> Self {
        *self.prepare.lock().expect("lock") = slug.to_owned();
        self
    }

    /// Serves `self` on `connection` under the real bus name and path.
    pub async fn serve(self, connection: &zbus::Connection) {
        connection
            .object_server()
            .at(porter_dbus::INFERENCE_PATH, self)
            .await
            .expect("serve the object");
        connection
            .request_name(porter_dbus::INFERENCE_BUS)
            .await
            .expect("own the name");
    }
}

#[zbus::interface(name = "org.quire.Inference1")]
impl FakeInferd {
    async fn prepare(
        &self,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<String> {
        let need = need_from_dbus(need).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        let text = |key: &str| {
            options
                .get(key)
                .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        };
        self.seen.lock().expect("lock").prepares.push(Opened {
            need,
            class,
            tier,
            traceparent: text(porter_dbus::OPTION_TRACEPARENT),
            usage: text(porter_dbus::OPTION_USAGE),
        });
        Ok(self.prepare.lock().expect("lock").clone())
    }

    async fn open(
        &self,
        need: NeedArg,
        class: String,
        tier: String,
        options: Details,
    ) -> fdo::Result<OwnedFd> {
        let need = need_from_dbus(need).map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        let spec = inferd::session::SessionSpec {
            need: need.clone(),
            class: parse_slug(&class)?,
            tier: parse_slug(&tier)?,
            usage: porter_core::consent::Usage::Interactive,
        };
        let traceparent = options
            .get(porter_dbus::OPTION_TRACEPARENT)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        let usage = options
            .get(porter_dbus::OPTION_USAGE)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        self.seen.lock().expect("lock").opens.push(Opened {
            need,
            class,
            tier,
            traceparent,
            usage,
        });
        let (ours, theirs) = StdStream::pair().map_err(|e| fdo::Error::Failed(e.to_string()))?;
        ours.set_nonblocking(true)
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let stream = UnixStream::from_std(ours).map_err(|e| fdo::Error::Failed(e.to_string()))?;
        match &self.behaviour {
            Behaviour::Served(served) => {
                let served = served.clone();
                tokio::spawn(async move {
                    let seams = inferd::serve::Seams {
                        router: served.route,
                        engines: served.gate,
                        runner: served.runner,
                        audit: super::served::Unaudited,
                    };
                    inferd::serve::serve_session(stream, spec, &seams).await;
                });
            }
            behaviour => {
                tokio::spawn(serve_session(
                    stream,
                    behaviour.clone(),
                    Arc::clone(&self.seen),
                ));
            }
        }
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(theirs)))
    }
}

fn parse_slug<T: serde::de::DeserializeOwned>(text: &str) -> fdo::Result<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))
}

async fn write_event(stream: &mut UnixStream, event: &InferEvent, dribble: bool) {
    let bytes = encode_frame(event).expect("encode");
    match dribble {
        false => {
            let _ = stream.write_all(&bytes).await;
        }
        true => {
            for byte in bytes {
                let _ = stream.write_all(&[byte]).await;
                let _ = stream.flush().await;
                tokio::task::yield_now().await;
            }
        }
    }
}

/// One read: bytes into `buffer`, memfd contents into `seen`; false at end of file.
async fn read_some(stream: &UnixStream, buffer: &mut Vec<u8>, seen: &Mutex<Seen>) -> bool {
    let result = stream
        .async_io(Interest::READABLE, || {
            let mut chunk = [0_u8; 4096];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(16))];
            let mut control = RecvAncillaryBuffer::new(&mut space);
            let got = recvmsg(
                stream,
                &mut [IoSliceMut::new(&mut chunk)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC,
            )
            .map_err(std::io::Error::from)?;
            let fds: Vec<_> = control
                .drain()
                .flat_map(|message| match message {
                    RecvAncillaryMessage::ScmRights(fds) => fds.collect::<Vec<_>>(),
                    _ => Vec::new(),
                })
                .collect();
            Ok((chunk[..got.bytes].to_vec(), fds))
        })
        .await;
    match result {
        Ok((bytes, fds)) if !bytes.is_empty() => {
            buffer.extend(bytes);
            fds.into_iter().for_each(|fd| {
                // The descriptor shares the sender's offset, which is at the end.
                let mut file = std::fs::File::from(fd);
                let mut text = Vec::new();
                let _ = std::io::Seek::rewind(&mut file);
                let _ = std::io::Read::read_to_end(&mut file, &mut text);
                seen.lock().expect("lock").attachments.push(text);
            });
            true
        }
        _ => false,
    }
}

async fn serve_session(mut stream: UnixStream, behaviour: Behaviour, seen: Arc<Mutex<Seen>>) {
    let (scripts, dribble) = match &behaviour {
        Behaviour::Scripted(scripts) => (scripts.clone(), false),
        Behaviour::Dribble(scripts) => (scripts.clone(), true),
        Behaviour::Refuse(refusal) => {
            let event = InferEvent::Finished(InferReply::Refused(*refusal));
            write_event(&mut stream, &event, false).await;
            return;
        }
        Behaviour::Raw(bytes) => {
            let _ = stream.write_all(bytes).await;
            let _ = stream.flush().await;
            // Keep the socket open until the client goes away.
            let mut sink = Vec::new();
            while read_some(&stream, &mut sink, &seen).await {}
            return;
        }
        Behaviour::HangUp => (Vec::new(), false),
        Behaviour::Served(_) => return,
    };
    let mut session = FakeInferSession::scripted(scripts);
    let mut buffer = Vec::new();
    loop {
        while let Ok(FrameRead::Complete(envelope, used)) = decode_frame::<ClientFrame>(&buffer) {
            buffer.drain(..used);
            seen.lock()
                .expect("lock")
                .frames
                .push(envelope.body.clone());
            if matches!(behaviour, Behaviour::HangUp) {
                return;
            }
            session.send(envelope.body).await.expect("fake session");
            while let Ok(event) = session.next().await {
                write_event(&mut stream, &event, dribble).await;
            }
        }
        if !read_some(&stream, &mut buffer, &seen).await {
            return;
        }
    }
}

/// The class and tier slugs the client must send for these values.
pub fn slugs(class: DataClass, tier: Tier) -> (String, String) {
    let slug = |value: serde_json::Value| value.as_str().map(str::to_owned).expect("slug");
    (
        slug(serde_json::to_value(class).expect("json")),
        slug(serde_json::to_value(tier).expect("json")),
    )
}

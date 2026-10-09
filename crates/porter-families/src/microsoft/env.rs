//! What a Microsoft provider needs from its surroundings, handed in rather than read where it is
//! used: the HTTP seam, the client registry, the build channel, the clock and randomness. Tests
//! hand in a fake issuer's seam and a counting clock; [`MicrosoftEnv::system`] is what accountd
//! and an app hosting porter in process use.

use crate::env_common::{ClientFiles, clients_now, system_now, system_random};
pub use crate::env_common::{Clock, Random};
use porter_http::Http;
use porter_oauth::ClientRegistry;
use porter_provider::ClientChannel;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

/// The surroundings of one Microsoft provider.
pub struct MicrosoftEnv<H> {
    /// Every call to the issuer and to Graph goes through it.
    pub http: Arc<H>,
    /// Which client id this build presents, when no clients files are read.
    pub registry: ClientRegistry,
    /// The clients files, read again at every use in place of `registry` (a daemon's: Settings
    /// writes the person's own while it runs).
    pub files: Option<ClientFiles>,
    /// Which build channel's client to look up.
    pub channel: ClientChannel,
    /// The time.
    pub clock: Clock,
    /// Randomness for PKCE.
    pub random: Random,
    /// How long one `Poll` waits before answering `Waiting`.
    pub poll_slice: Duration,
}

impl<H> Clone for MicrosoftEnv<H> {
    fn clone(&self) -> Self {
        Self {
            http: Arc::clone(&self.http),
            registry: self.registry.clone(),
            files: self.files.clone(),
            channel: self.channel,
            clock: Arc::clone(&self.clock),
            random: Arc::clone(&self.random),
            poll_slice: self.poll_slice,
        }
    }
}

impl<H> std::fmt::Debug for MicrosoftEnv<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftEnv")
            .field("channel", &self.channel)
            .field("poll_slice", &self.poll_slice)
            .finish_non_exhaustive()
    }
}

impl<H: Http> MicrosoftEnv<H> {
    /// An environment over `http` with the system's clock and randomness, `registry` for the
    /// clients, the stable channel and a two-second poll.
    pub fn new(http: H, registry: ClientRegistry) -> Self {
        Self {
            http: Arc::new(http),
            registry,
            files: None,
            channel: ClientChannel::Stable,
            clock: Arc::new(system_now),
            random: Arc::new(system_random),
            poll_slice: Duration::from_secs(2),
        }
    }

    /// Reading the clients from `files` at every use, in place of the registry it was made with.
    pub fn with_files(self, files: ClientFiles) -> Self {
        Self {
            files: Some(files),
            ..self
        }
    }

    /// With this build channel.
    pub fn with_channel(self, channel: ClientChannel) -> Self {
        Self { channel, ..self }
    }

    /// With this clock.
    pub fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    /// With this randomness.
    pub fn with_random(self, random: Random) -> Self {
        Self { random, ..self }
    }

    /// With this poll wait.
    pub fn with_poll_slice(self, poll_slice: Duration) -> Self {
        Self { poll_slice, ..self }
    }
}

impl<H> MicrosoftEnv<H> {
    /// The clients as they are now: the files' when it reads files, else its registry.
    pub fn clients(&self) -> Cow<'_, ClientRegistry> {
        clients_now(self.files.as_ref(), &self.registry)
    }
}

impl<H: Http + Default> MicrosoftEnv<H> {
    /// The environment of a daemon: the shipped clients file and the person's own, found from
    /// `XDG_CONFIG_HOME` and `HOME`, read again at every use. A damaged clients file reads as
    /// empty, so the sign-in says it needs a client id rather than guessing one.
    pub fn system() -> Self {
        Self::with_client_files(ClientFiles::system())
    }

    /// The environment of a daemon reading its clients from `files`.
    pub fn with_client_files(files: ClientFiles) -> Self {
        Self::new(H::default(), ClientRegistry::default()).with_files(files)
    }
}

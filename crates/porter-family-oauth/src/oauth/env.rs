//! What an OAuth provider needs from its surroundings, handed in rather than read where it is
//! used: the HTTP seam, the client registry, the build channel, the clock, randomness and where
//! the account's identity is read. Tests hand in a fake issuer's seam and a counting clock; the
//! caller hands in its clock and the clients files, and [`OAuthEnv::with_client_files`] is what
//! accountd uses.
//!
//! The installed-app flow is the loopback redirect only: there is no device-code path (Google's
//! device flow serves a short list of scopes that has none of Calendar, People or Tasks, and
//! nothing here can tell that no browser is available, because the host opens the page and a
//! failure to open it is not reported back).

use crate::env_common::{ClientFiles, Clock, Random, clients_now, clock_of, system_random};
use porter_core::EndpointUrl;
use porter_http::Http;
use porter_oauth::ClientRegistry;
use porter_provider::{ClientChannel, Issuer};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

/// The surroundings of one OAuth provider.
pub struct OAuthEnv<H> {
    /// Every call to the issuer and to its APIs goes through it.
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
    /// Where the account's address and name are read instead of the issuer's own userinfo
    /// endpoint (a fake issuer in a test).
    pub userinfo: Option<EndpointUrl>,
    /// How long one `Poll` waits before answering `Waiting`.
    pub poll_slice: Duration,
}

impl<H> Clone for OAuthEnv<H> {
    fn clone(&self) -> Self {
        Self {
            http: Arc::clone(&self.http),
            registry: self.registry.clone(),
            files: self.files.clone(),
            channel: self.channel,
            clock: Arc::clone(&self.clock),
            random: Arc::clone(&self.random),
            userinfo: self.userinfo.clone(),
            poll_slice: self.poll_slice,
        }
    }
}

impl<H> std::fmt::Debug for OAuthEnv<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthEnv")
            .field("channel", &self.channel)
            .field("userinfo", &self.userinfo)
            .field("poll_slice", &self.poll_slice)
            .finish_non_exhaustive()
    }
}

impl<H: Http> OAuthEnv<H> {
    /// An environment over `http` with `clock` for the time and the system's randomness,
    /// `registry` for the clients, the stable channel, the issuer's own userinfo endpoint and a
    /// two-second poll.
    pub fn new<C: porter_ids::clock::Clock + 'static>(
        http: H,
        registry: ClientRegistry,
        clock: C,
    ) -> Self {
        Self {
            http: Arc::new(http),
            registry,
            files: None,
            channel: ClientChannel::Stable,
            clock: clock_of(clock),
            random: Arc::new(system_random),
            userinfo: None,
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

    /// Reading the account's identity from `userinfo` (a fake issuer in a test).
    pub fn with_userinfo(self, userinfo: EndpointUrl) -> Self {
        Self {
            userinfo: Some(userinfo),
            ..self
        }
    }
}

impl<H> OAuthEnv<H> {
    /// The clients as they are now: the files' when it reads files, else its registry.
    pub fn clients(&self) -> Cow<'_, ClientRegistry> {
        clients_now(self.files.as_ref(), &self.registry)
    }

    /// Where `issuer`'s account identity is read: the one this environment was given, else the
    /// issuer's own userinfo endpoint, if it publishes one.
    pub(crate) fn userinfo_of(&self, issuer: Issuer) -> Option<EndpointUrl> {
        self.userinfo
            .clone()
            .or_else(|| issuer.data().userinfo.clone())
    }
}

impl<H: Http + Default> OAuthEnv<H> {
    /// The environment of a daemon reading its clients from `files` (the shipped clients file
    /// and the person's own, read again at every use: a damaged file reads as empty, so the
    /// sign-in says it needs a client id rather than guessing one) and the time from `clock`.
    pub fn with_client_files<C: porter_ids::clock::Clock + 'static>(
        files: ClientFiles,
        clock: C,
    ) -> Self {
        Self::new(H::default(), ClientRegistry::default(), clock).with_files(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::HttpError;
    use porter_http::http::{Request, Response};

    #[derive(Default)]
    struct Nowhere;

    impl Http for Nowhere {
        async fn send(&self, _: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
            Err(HttpError::Unreachable)
        }
    }

    fn env() -> OAuthEnv<Nowhere> {
        OAuthEnv::new(
            Nowhere,
            ClientRegistry::default(),
            porter_ids::clock::FixedClock::new(porter_ids::UnixSeconds(0)),
        )
    }

    #[test]
    fn the_defaults_are_the_issuers_own_userinfo_and_the_stable_channel() {
        let env = env();
        assert_eq!(env.channel, ClientChannel::Stable);
        let google = env
            .userinfo_of(Issuer::Google)
            .expect("google publishes one");
        assert!(google.as_str().contains("googleapis.com"));
        assert_eq!(env.userinfo_of(Issuer::Dropbox), None);
        let moved = env.with_userinfo(
            EndpointUrl::parse("http://127.0.0.1:1/v1/userinfo").expect("a literal url"),
        );
        let moved = moved.userinfo_of(Issuer::Google).expect("given");
        assert!(moved.as_str().starts_with("http://127.0.0.1"));
    }
}

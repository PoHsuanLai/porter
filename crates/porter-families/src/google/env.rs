//! What a Google provider needs from its surroundings, handed in rather than read where it is
//! used: the HTTP seam, the client registry, the build channel, the clock, randomness and where
//! the account's identity is read. Tests hand in a fake Google's seam and a counting clock; the
//! caller hands in its clock and the clients files, and [`GoogleEnv::with_client_files`] is what
//! accountd uses.
//!
//! Google's installed-app flow is the loopback redirect only: its device flow serves a short
//! list of scopes that has none of Calendar, People or Tasks, so there is no device-code path.

use crate::env_common::{ClientFiles, Clock, Random, clock_of, clients_now, system_random};
use porter_core::EndpointUrl;
use porter_http::Http;
use porter_oauth::ClientRegistry;
use porter_provider::ClientChannel;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

/// OpenID Connect's userinfo endpoint, where the account's address and name are read.
const USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// The surroundings of one Google provider.
pub struct GoogleEnv<H> {
    /// Every call to the issuer and to Google's APIs goes through it.
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
    /// Where the account's address and name are read.
    pub userinfo: EndpointUrl,
    /// How long one `Poll` waits before answering `Waiting`.
    pub poll_slice: Duration,
}

impl<H> Clone for GoogleEnv<H> {
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

impl<H> std::fmt::Debug for GoogleEnv<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleEnv")
            .field("channel", &self.channel)
            .field("userinfo", &self.userinfo)
            .field("poll_slice", &self.poll_slice)
            .finish_non_exhaustive()
    }
}

impl<H: Http> GoogleEnv<H> {
    /// An environment over `http` with `clock` for the time and the system's randomness,
    /// `registry` for the clients, the stable channel, Google's userinfo endpoint and a two-second
    /// poll.
    pub fn new<C: porter_core::clock::Clock + 'static>(
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
            userinfo: userinfo_endpoint(USERINFO),
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

    /// Reading the account's identity from `userinfo` (a fake Google in a test).
    pub fn with_userinfo(self, userinfo: EndpointUrl) -> Self {
        Self { userinfo, ..self }
    }
}

impl<H> GoogleEnv<H> {
    /// The clients as they are now: the files' when it reads files, else its registry.
    pub fn clients(&self) -> Cow<'_, ClientRegistry> {
        clients_now(self.files.as_ref(), &self.registry)
    }
}

impl<H: Http + Default> GoogleEnv<H> {
    /// The environment of a daemon reading its clients from `files` (the shipped clients file
    /// and the person's own, read again at every use: a damaged file reads as empty, so the
    /// sign-in says it needs a client id rather than guessing one) and the time from `clock`.
    pub fn with_client_files<C: porter_core::clock::Clock + 'static>(
        files: ClientFiles,
        clock: C,
    ) -> Self {
        Self::new(H::default(), ClientRegistry::default(), clock).with_files(files)
    }
}

fn userinfo_endpoint(url: &str) -> EndpointUrl {
    EndpointUrl::parse(url).unwrap_or_else(|_| unreachable!("{url} is a literal endpoint URL"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_http::{HttpError, HttpRequest, HttpResponse};

    #[derive(Default)]
    struct Nowhere;

    impl Http for Nowhere {
        async fn send(&self, _: HttpRequest) -> Result<HttpResponse, HttpError> {
            Err(HttpError::Unreachable)
        }
    }

    #[test]
    fn the_defaults_are_googles_and_the_stable_channel() {
        let env = GoogleEnv::new(
            Nowhere,
            ClientRegistry::default(),
            porter_core::clock::FixedClock::new(porter_core::UnixSeconds(0)),
        );
        assert_eq!(env.userinfo.as_str(), USERINFO);
        assert_eq!(env.channel, ClientChannel::Stable);
        let moved = env.with_userinfo(userinfo_endpoint("http://127.0.0.1:1/v1/userinfo"));
        assert!(moved.userinfo.as_str().starts_with("http://127.0.0.1"));
    }

    /// ux-3: Google is on the add list only once a Google client is registered.
    #[test]
    fn google_is_ready_only_with_a_client_of_its_own_channel() {
        use porter_provider::{
            ClientEntry, ClientId, ClientsFile, Issuer, Provider, Readiness, shipped_specs,
        };
        let spec = shipped_specs()
            .into_iter()
            .find(|s| s.id.as_str() == "google")
            .expect("google");
        let with = |channel| {
            ClientRegistry::layered(
                ClientsFile {
                    clients: vec![ClientEntry {
                        issuer: Issuer::Google,
                        channel,
                        client_id: ClientId("mine.apps.googleusercontent.com".into()),
                        client_secret: None,
                        endpoints: None,
                    }],
                },
                ClientsFile::default(),
            )
        };
        let cases = [
            ("none", ClientRegistry::default(), Readiness::NeedsClient),
            (
                "another channel",
                with(ClientChannel::Development),
                Readiness::NeedsClient,
            ),
            ("its own", with(ClientChannel::Stable), Readiness::Ready),
        ];
        for (name, registry, want) in cases {
            let provider = super::super::GoogleProvider::with_env(
                spec.clone(),
                GoogleEnv::new(
                    Nowhere,
                    registry,
                    porter_core::clock::FixedClock::new(porter_core::UnixSeconds(0)),
                ),
            );
            assert_eq!(provider.readiness(), want, "{name}");
        }
    }
}

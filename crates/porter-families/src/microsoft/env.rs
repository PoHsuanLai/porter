//! What a Microsoft provider needs from its surroundings, handed in rather than read where it is
//! used: the HTTP seam, the client registry, the build channel, the clock, randomness and how a
//! sign-in is asked to proceed. Tests hand in a fake issuer's seam and a counting clock;
//! [`MicrosoftEnv::system`] is what accountd and an app hosting porter in process use.

use porter_core::UnixSeconds;
use porter_http::Http;
use porter_oauth::ClientRegistry;
use porter_provider::ClientChannel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How the person is asked to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignInFlow {
    /// The browser on this computer, redirecting to a loopback listener (PKCE, S256).
    #[default]
    Loopback,
    /// A code typed on another device (headless and SSH sessions).
    DeviceCode,
}

/// A source of the current time.
pub type Clock = Arc<dyn Fn() -> UnixSeconds + Send + Sync>;

/// A source of the PKCE verifier's 32 bytes and the state's 16, or `None` when the system has
/// no randomness to give (the sign-in then fails; it never falls back to something guessable).
pub type Random = Arc<dyn Fn() -> Option<([u8; 32], [u8; 16])> + Send + Sync>;

/// The surroundings of one Microsoft provider.
pub struct MicrosoftEnv<H> {
    /// Every call to the issuer and to Graph goes through it.
    pub http: Arc<H>,
    /// Which client id this build presents.
    pub registry: ClientRegistry,
    /// Which build channel's client to look up.
    pub channel: ClientChannel,
    /// The time.
    pub clock: Clock,
    /// Randomness for PKCE.
    pub random: Random,
    /// The sign-in flow to start.
    pub flow: SignInFlow,
    /// How long one `Poll` waits before answering `Waiting`.
    pub poll_slice: Duration,
}

impl<H> Clone for MicrosoftEnv<H> {
    fn clone(&self) -> Self {
        Self {
            http: Arc::clone(&self.http),
            registry: self.registry.clone(),
            channel: self.channel,
            clock: Arc::clone(&self.clock),
            random: Arc::clone(&self.random),
            flow: self.flow,
            poll_slice: self.poll_slice,
        }
    }
}

impl<H> std::fmt::Debug for MicrosoftEnv<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftEnv")
            .field("channel", &self.channel)
            .field("flow", &self.flow)
            .field("poll_slice", &self.poll_slice)
            .finish_non_exhaustive()
    }
}

impl<H: Http> MicrosoftEnv<H> {
    /// An environment over `http` with the system's clock and randomness, `registry` for the
    /// clients, the stable channel, the loopback flow and a two-second poll.
    pub fn new(http: H, registry: ClientRegistry) -> Self {
        Self {
            http: Arc::new(http),
            registry,
            channel: ClientChannel::Stable,
            clock: Arc::new(system_now),
            random: Arc::new(system_random),
            flow: SignInFlow::default(),
            poll_slice: Duration::from_secs(2),
        }
    }

    /// With this build channel.
    pub fn with_channel(self, channel: ClientChannel) -> Self {
        Self { channel, ..self }
    }

    /// With this sign-in flow.
    pub fn with_flow(self, flow: SignInFlow) -> Self {
        Self { flow, ..self }
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

impl<H: Http + Default> MicrosoftEnv<H> {
    /// The environment of a daemon: the shipped clients file and the person's own, found from
    /// `XDG_CONFIG_HOME` and `HOME`. A damaged clients file leaves the registry empty, so the
    /// sign-in says it needs a client id rather than guessing one.
    pub fn system() -> Self {
        let own = own_clients_path(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        );
        let registry =
            ClientRegistry::from_paths(Path::new(SHIPPED_CLIENTS), &own).unwrap_or_default();
        Self::new(H::default(), registry)
    }
}

/// Where packaging ships the clients file.
const SHIPPED_CLIENTS: &str = "/usr/share/porter/clients.toml";

/// The person's own clients file: under `XDG_CONFIG_HOME` when it is an absolute path, else under
/// `HOME/.config`.
pub(super) fn own_clients_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    let base = xdg
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"));
    base.join("porter").join("clients.toml")
}

fn system_now() -> UnixSeconds {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    UnixSeconds(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// 48 bytes from the operating system's `/dev/urandom`.
fn system_random() -> Option<([u8; 32], [u8; 16])> {
    use std::io::Read;
    let mut bytes = [0u8; 48];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .ok()?;
    let (verifier, state) = bytes.split_at(32);
    Some((verifier.try_into().ok()?, state.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_clients_file_follows_xdg_then_home() {
        const CASES: &[(&str, Option<&str>, Option<&str>, &str)] = &[
            (
                "xdg wins",
                Some("/x/cfg"),
                Some("/home/a"),
                "/x/cfg/porter/clients.toml",
            ),
            (
                "home when no xdg",
                None,
                Some("/home/a"),
                "/home/a/.config/porter/clients.toml",
            ),
            (
                "relative xdg is ignored",
                Some("cfg"),
                Some("/home/a"),
                "/home/a/.config/porter/clients.toml",
            ),
            ("neither", None, None, "/nonexistent/porter/clients.toml"),
        ];
        for (name, xdg, home, want) in CASES {
            let got = own_clients_path(xdg.map(PathBuf::from), home.map(PathBuf::from));
            assert_eq!(got, PathBuf::from(want), "{name}");
        }
    }

    #[test]
    fn the_system_random_gives_fresh_bytes() {
        let a = system_random().expect("urandom");
        let b = system_random().expect("urandom");
        assert_ne!(a, b);
    }
}

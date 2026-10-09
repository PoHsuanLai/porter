//! What the OAuth families (Microsoft, Google) take from their surroundings: the time, randomness
//! for PKCE, and the clients files. The caller hands in the clock (a `porter_core::clock::Clock`)
//! and the clients files' paths; the families read neither the environment nor the system clock.
//! Randomness is still read from the operating system. Each family's environment holds these as
//! arguments, so a test hands in a counting clock and fixed bytes.

use porter_core::UnixSeconds;
use porter_core::clock::Clock as ClockSource;
use std::path::PathBuf;
use std::sync::Arc;

/// A source of the current time, as a closure: what a family reads the time through. A caller
/// with a `porter_core::clock::Clock` converts it with [`clock_of`].
pub type Clock = Arc<dyn Fn() -> UnixSeconds + Send + Sync>;

/// The closure a family reads the time through, over a caller's clock.
pub(crate) fn clock_of<C: ClockSource + 'static>(clock: C) -> Clock {
    Arc::new(move || clock.now())
}

/// A source of the PKCE verifier's 32 bytes and the state's 16, or `None` when the system has
/// no randomness to give (the sign-in then fails; it never falls back to something guessable).
pub type Random = Arc<dyn Fn() -> Option<([u8; 32], [u8; 16])> + Send + Sync>;

/// The two clients files a daemon's families read: the shipped one and the person's own, which
/// Settings writes while the daemon runs. They are read again for every sign-in and every token,
/// so a client id set in Settings is used by the next sign-in without a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFiles {
    /// The shipped file (`/usr/share/porter/clients.toml`).
    pub shipped: PathBuf,
    /// The person's own (`$XDG_CONFIG_HOME/porter/clients.toml`).
    pub own: PathBuf,
}

impl ClientFiles {
    /// The clients the files name now. A damaged file leaves the registry empty, so a sign-in
    /// says it needs a client id rather than guessing one.
    pub fn read(&self) -> porter_oauth::ClientRegistry {
        porter_oauth::ClientRegistry::from_paths(&self.shipped, &self.own).unwrap_or_default()
    }
}

/// The clients an environment presents: the files' as they are now when it reads files, else the
/// registry it was handed.
pub(crate) fn clients_now<'a>(
    files: Option<&ClientFiles>,
    registry: &'a porter_oauth::ClientRegistry,
) -> std::borrow::Cow<'a, porter_oauth::ClientRegistry> {
    match files {
        Some(files) => std::borrow::Cow::Owned(files.read()),
        None => std::borrow::Cow::Borrowed(registry),
    }
}

/// 48 bytes from the operating system's `/dev/urandom`.
pub(crate) fn system_random() -> Option<([u8; 32], [u8; 16])> {
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
    fn the_files_are_read_again_each_time_so_a_row_written_later_is_seen() {
        let dir = std::env::temp_dir().join(format!("families-clients-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let files = ClientFiles {
            shipped: dir.join("absent.toml"),
            own: dir.join("clients.toml"),
        };
        let fixed = porter_oauth::ClientRegistry::default();
        let lookup = |r: &porter_oauth::ClientRegistry| {
            r.lookup(
                porter_provider::Issuer::Microsoft,
                porter_provider::ClientChannel::Stable,
            )
            .map(|c| c.client_id.0.clone())
        };
        assert_eq!(lookup(&clients_now(Some(&files), &fixed)), None);
        std::fs::write(
            &files.own,
            "[[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"mine\"\n",
        )
        .expect("write");
        assert_eq!(
            lookup(&clients_now(Some(&files), &fixed)).as_deref(),
            Some("mine")
        );
        // An environment handed a registry keeps it.
        assert_eq!(lookup(&clients_now(None, &fixed)), None);
        let _ = std::fs::remove_file(&files.own);
    }

    #[test]
    fn the_system_random_gives_fresh_bytes() {
        let a = system_random().expect("urandom");
        let b = system_random().expect("urandom");
        assert_ne!(a, b);
    }
}

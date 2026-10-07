//! What the OAuth families (Microsoft, Google) take from the machine rather than from a caller:
//! the time, randomness for PKCE, and where the clients files are. Each family's environment
//! holds these as arguments, so a test hands in a counting clock and fixed bytes.

use porter_core::UnixSeconds;
use std::path::PathBuf;
use std::sync::Arc;

/// A source of the current time.
pub type Clock = Arc<dyn Fn() -> UnixSeconds + Send + Sync>;

/// A source of the PKCE verifier's 32 bytes and the state's 16, or `None` when the system has
/// no randomness to give (the sign-in then fails; it never falls back to something guessable).
pub type Random = Arc<dyn Fn() -> Option<([u8; 32], [u8; 16])> + Send + Sync>;

/// Where packaging ships the clients file.
pub(crate) const SHIPPED_CLIENTS: &str = "/usr/share/porter/clients.toml";

/// The person's own clients file: under `XDG_CONFIG_HOME` when it is an absolute path, else under
/// `HOME/.config`.
pub(crate) fn own_clients_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    let base = xdg
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"));
    base.join("porter").join("clients.toml")
}

pub(crate) fn system_now() -> UnixSeconds {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    UnixSeconds(i64::try_from(seconds).unwrap_or(i64::MAX))
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

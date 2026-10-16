//! Which distro this is and whether it packages an engine: the `os-release` file read for the
//! few words a package choice needs. A package is only ever named for a release that was
//! checked to ship it.

/// The words of `os-release` a package choice needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    /// `ID`: `fedora`, `ubuntu`, `arch`.
    pub id: String,
    /// `ID_LIKE`, one word each: `debian` for Ubuntu, `arch` for Manjaro.
    pub like: Vec<String>,
    /// `VERSION_ID` as (major, minor): `26.04` is (26, 4), `43` is (43, 0). `None` for a
    /// rolling or testing release that has none.
    pub version: Option<(u32, u32)>,
}

impl OsRelease {
    /// The file's text, read leniently: a line that is not `KEY=value` is skipped.
    pub fn parse(text: &str) -> Self {
        let value = |key: &str| {
            text.lines()
                .filter_map(|line| line.split_once('='))
                .find(|(name, _)| name.trim() == key)
                .map(|(_, value)| value.trim().trim_matches(['"', '\'']).to_owned())
        };
        Self {
            id: value("ID").unwrap_or_default().to_ascii_lowercase(),
            like: value("ID_LIKE")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect(),
            version: value("VERSION_ID").and_then(|text| {
                let mut parts = text.split('.');
                let major = parts.next()?.parse().ok()?;
                let minor = parts.next().map_or(Some(0), |part| part.parse().ok())?;
                Some((major, minor))
            }),
        }
    }
}

/// The program that installs packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// Debian and Ubuntu.
    Apt,
    /// Fedora.
    Dnf,
    /// Arch.
    Pacman,
}

impl PackageManager {
    /// The program, by its fixed absolute path: nothing a caller or the environment names.
    pub fn program(self) -> &'static str {
        match self {
            Self::Apt => "/usr/bin/apt-get",
            Self::Dnf => "/usr/bin/dnf",
            Self::Pacman => "/usr/bin/pacman",
        }
    }

    /// The arguments before the package name: install without asking.
    pub fn install_args(self) -> &'static [&'static str] {
        match self {
            Self::Apt => &["install", "-y"],
            Self::Dnf => &["install", "-y"],
            Self::Pacman => &["-S", "--noconfirm", "--needed"],
        }
    }
}

/// Which releases a package is known to be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Releases {
    /// Releases whose `ID` is this one, from this version on.
    From {
        /// `ID`.
        id: &'static str,
        /// The first release, (major, minor).
        version: (u32, u32),
        /// Whether a release with no version number (Debian testing) counts as new enough.
        unnumbered: Unnumbered,
    },
    /// Any release whose `ID` or `ID_LIKE` is this one (a rolling distro and its derivatives).
    Rolling {
        /// `ID` or one word of `ID_LIKE`.
        id: &'static str,
    },
}

/// How a release with no `VERSION_ID` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unnumbered {
    /// As new: Debian testing is ahead of every stable release.
    New,
    /// As old: no package can be promised.
    Old,
}

/// One distro package that holds an engine, and the releases it was checked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageSource {
    /// The manager that installs it.
    pub manager: PackageManager,
    /// The package's name.
    pub name: &'static str,
    /// Where it exists.
    pub releases: Releases,
    /// Whether the package's file list was checked to hold the engine's program at its usual
    /// path. A row that is not is installed all the same and the program looked for after.
    pub verified: Verified,
}

/// Whether a manifest row was checked against what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    /// Checked on 2026-10-17 against the distro's own package listing.
    Yes,
    /// Not checked; see the row's comment.
    No,
}

impl PackageSource {
    /// Whether this package is known to exist on `release`.
    pub fn exists_on(&self, release: &OsRelease) -> bool {
        match self.releases {
            Releases::Rolling { id } => release.id == id || release.like.iter().any(|l| l == id),
            Releases::From {
                id,
                version,
                unnumbered,
            } => {
                release.id == id
                    && match release.version {
                        Some(found) => found >= version,
                        None => unnumbered == Unnumbered::New,
                    }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU_24: &str = "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"24.04\"\n";

    fn from(id: &'static str, version: (u32, u32), unnumbered: Unnumbered) -> PackageSource {
        PackageSource {
            manager: PackageManager::Apt,
            name: "x",
            releases: Releases::From {
                id,
                version,
                unnumbered,
            },
            verified: Verified::Yes,
        }
    }

    #[test]
    fn os_release_is_read_for_id_like_and_version() {
        let release = OsRelease::parse(UBUNTU_24);
        assert_eq!(release.id, "ubuntu");
        assert_eq!(release.like, ["debian"]);
        assert_eq!(release.version, Some((24, 4)));
        assert_eq!(
            OsRelease::parse("ID=fedora\nVERSION_ID=43\n").version,
            Some((43, 0))
        );
        assert_eq!(OsRelease::parse("ID=arch\n").version, None);
        assert_eq!(OsRelease::parse(""), OsRelease::default());
    }

    #[test]
    fn a_versioned_package_exists_only_on_its_own_id_and_new_enough_releases() {
        let ubuntu_26 = from("ubuntu", (26, 4), Unnumbered::Old);
        assert!(!ubuntu_26.exists_on(&OsRelease::parse(UBUNTU_24)));
        assert!(ubuntu_26.exists_on(&OsRelease::parse("ID=ubuntu\nVERSION_ID=\"26.04\"\n")));
        // Ubuntu says it is like Debian; a Debian number means nothing for it.
        let debian_14 = from("debian", (14, 0), Unnumbered::New);
        assert!(!debian_14.exists_on(&OsRelease::parse(UBUNTU_24)));
        assert!(debian_14.exists_on(&OsRelease::parse("ID=debian\n")));
        assert!(
            !from("debian", (14, 0), Unnumbered::Old).exists_on(&OsRelease::parse("ID=debian\n"))
        );
    }

    #[test]
    fn a_rolling_package_exists_on_the_distro_and_its_derivatives() {
        let arch = PackageSource {
            releases: Releases::Rolling { id: "arch" },
            ..from("x", (0, 0), Unnumbered::Old)
        };
        assert!(arch.exists_on(&OsRelease::parse("ID=arch\n")));
        assert!(arch.exists_on(&OsRelease::parse("ID=manjaro\nID_LIKE=arch\n")));
        assert!(!arch.exists_on(&OsRelease::parse(UBUNTU_24)));
    }
}

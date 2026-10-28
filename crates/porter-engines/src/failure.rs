//! Why an install did not happen, as a code; the words for it are the settings page's.

/// Why an install ended without an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// Not enough free space for the download and its unpacking.
    NoSpace,
    /// The download could not be finished: no connection, or it broke off.
    Offline,
    /// What arrived is not what the manifest pins, so nothing was installed.
    Corrupt,
    /// The person did not give the administrator's password.
    Declined,
    /// Nothing is pinned for this computer.
    Unsupported,
    /// The system's package manager could not install the package.
    Package,
    /// The person stopped it.
    Cancelled,
    /// Anything else.
    Other,
}

impl Failure {
    /// The word on the bus and on the helper's report line.
    pub fn slug(self) -> &'static str {
        match self {
            Self::NoSpace => "no_space",
            Self::Offline => "offline",
            Self::Corrupt => "corrupt",
            Self::Declined => "declined",
            Self::Unsupported => "unsupported",
            Self::Package => "package",
            Self::Cancelled => "cancelled",
            Self::Other => "other",
        }
    }

    /// The failure a word names; `None` for any other text.
    pub fn from_slug(text: &str) -> Option<Self> {
        [
            Self::NoSpace,
            Self::Offline,
            Self::Corrupt,
            Self::Declined,
            Self::Unsupported,
            Self::Package,
            Self::Cancelled,
            Self::Other,
        ]
        .into_iter()
        .find(|failure| failure.slug() == text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_a_slug_that_names_it() {
        for failure in [
            Failure::NoSpace,
            Failure::Offline,
            Failure::Corrupt,
            Failure::Declined,
            Failure::Unsupported,
            Failure::Package,
            Failure::Cancelled,
            Failure::Other,
        ] {
            assert_eq!(Failure::from_slug(failure.slug()), Some(failure));
        }
        assert_eq!(Failure::from_slug("sudo"), None);
    }
}

//! Why an install did not happen, in the words a person reads.

/// Why an install ended without an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The sentence the person reads. No path, no program name, no protocol.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::NoSpace => "There isn't enough free space on this computer to download it.",
            Self::Offline => {
                "The download didn't finish. Check your internet connection and try again."
            }
            Self::Corrupt => {
                "What was downloaded wasn't what was expected, so it wasn't installed."
            }
            Self::Declined => "It needs an administrator's password to install for all users.",
            Self::Unsupported => "There is no download of it for this computer.",
            Self::Package => "This computer's software installer couldn't install it.",
            Self::Cancelled => "The download was stopped.",
            Self::Other => "It couldn't be installed.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_a_slug_that_names_it_and_a_sentence_with_no_jargon() {
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
            let said = failure.sentence().to_lowercase();
            for word in ["sha", "http", "tar", "root", "pkexec", "polkit", "/"] {
                assert!(!said.contains(word), "{said}");
            }
        }
        assert_eq!(Failure::from_slug("sudo"), None);
    }
}

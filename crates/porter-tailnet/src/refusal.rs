//! Why a computer that asked to use this computer's models was not let in, in the words the
//! person reads. The refusal travels back to the asking computer, so each sentence is written
//! from its side: "that computer" is the one that lends, "this computer" the one that asked.

/// Why a request was refused. `Display` is the plain sentence; no program name, no network
/// term, and every sentence ends with a full stop.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Refusal {
    /// The request came from the lending computer itself.
    #[error("That computer won't answer a request that came from itself.")]
    ThisComputer,
    /// Tailscale does not know who sent it.
    #[error("That computer could not tell which of your computers is asking.")]
    Unknown,
    /// Tailscale could not be asked.
    #[error("That computer could not check who is asking.")]
    CouldNotCheck,
    /// The asking computer is a tagged server.
    #[error("That computer won't lend its models to this one, which is set up as a server.")]
    Tagged,
    /// The asking computer is another person's.
    #[error(
        "That computer won't lend its models to this one, which belongs to someone else, unless it is allowed in Settings there."
    )]
    Shared,
    /// The person said no to it.
    #[error(
        "That computer was told not to let this one use its models. This can be changed in Settings there."
    )]
    Denied,
    /// The person has not answered yet.
    #[error(
        "That computer is asking its owner whether to let this one use its models. Say yes there, then try again."
    )]
    Waiting,
    /// Too many computers are waiting for an answer.
    #[error("That computer is already asking about several computers. Answer those first.")]
    TooManyAsking,
    /// The answer could not be kept.
    #[error("That computer could not keep the answer. Try again.")]
    NotKept,
    /// The asking computer is already using as much as it may.
    #[error("That computer is busy with this one's other requests. Try again in a moment.")]
    Busy,
    /// Lending is off.
    #[error("That computer isn't lending its models.")]
    Off,
}

impl Refusal {
    /// The HTTP status the refusal is answered with.
    pub fn status(&self) -> u16 {
        match self {
            Refusal::Busy | Refusal::TooManyAsking => 429,
            Refusal::Off | Refusal::CouldNotCheck | Refusal::NotKept => 503,
            _ => 403,
        }
    }

    /// Every refusal, for tables.
    pub const ALL: [Refusal; 11] = [
        Refusal::ThisComputer,
        Refusal::Unknown,
        Refusal::CouldNotCheck,
        Refusal::Tagged,
        Refusal::Shared,
        Refusal::Denied,
        Refusal::Waiting,
        Refusal::TooManyAsking,
        Refusal::NotKept,
        Refusal::Busy,
        Refusal::Off,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words a person reads never include a program name or a term of the network's plumbing.
    const JARGON: [&str; 16] = [
        "porter", "inferd", "accountd", "socket", "port ", "node", "tailnet", "address",
        "localapi", "daemon", "whois", "http", "tcp", "listener", "tagged", "api",
    ];

    #[test]
    fn every_refusal_reads_as_plain_words() {
        for refusal in Refusal::ALL {
            let text = refusal.to_string();
            assert!(text.ends_with('.'), "{text}");
            let lower = text.to_lowercase();
            for word in JARGON {
                assert!(!lower.contains(word), "{text:?} says {word:?}");
            }
        }
    }

    #[test]
    fn what_is_waiting_or_busy_is_not_a_forbidden() {
        assert_eq!(Refusal::Waiting.status(), 403);
        assert_eq!(Refusal::Busy.status(), 429);
        assert_eq!(Refusal::Off.status(), 503);
        assert_eq!(Refusal::Denied.status(), 403);
    }
}

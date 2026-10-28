//! The words a person reads about the Tailscale network, written from the typed reasons the
//! libraries return (`porter_tailnet::Refusal`, `DialError`, `porter_tailscale::TailscaleError`).
//! inferd owns the settings page and the answers to the asking computer, so the sentences live
//! here and nowhere in those libraries. A refusal travels back to the asking computer, so each
//! sentence is written from its side: "that computer" is the one that lends, "this computer"
//! the one that asked.

use porter_tailnet::{DialError, Refusal};
use porter_tailscale::TailscaleError;

/// Why a request was refused, as a plain sentence ending in a full stop.
pub fn refusal(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::ThisComputer => "That computer won't answer a request that came from itself.",
        Refusal::Unknown => "That computer could not tell which of your computers is asking.",
        Refusal::CouldNotCheck => "That computer could not check who is asking.",
        Refusal::Tagged => {
            "That computer won't lend its models to this one, which is set up as a server."
        }
        Refusal::Shared => {
            "That computer won't lend its models to this one, which belongs to someone else, unless it is allowed in Settings there."
        }
        Refusal::Denied => {
            "That computer was told not to let this one use its models. This can be changed in Settings there."
        }
        Refusal::Waiting => {
            "That computer is asking its owner whether to let this one use its models. Say yes there, then try again."
        }
        Refusal::TooManyAsking => {
            "That computer is already asking about several computers. Answer those first."
        }
        Refusal::NotKept => "That computer could not keep the answer. Try again.",
        Refusal::Busy => {
            "That computer is busy with this one's other requests. Try again in a moment."
        }
        Refusal::Off => "That computer isn't lending its models.",
        // a refusal a newer library adds: the generic one
        _ => "That computer won't lend its models to this one.",
    }
}

/// Why Tailscale could not be asked.
fn tailscale(error: &TailscaleError) -> &'static str {
    match error {
        TailscaleError::NotInstalled => "Tailscale isn't installed on this computer.",
        TailscaleError::NotRunning => "Tailscale isn't running on this computer.",
        TailscaleError::Refused => {
            "This computer's Tailscale doesn't let your accounts ask it yet."
        }
        TailscaleError::SignedOut => "Tailscale is signed out.",
        TailscaleError::NoSuchPeer => "Tailscale does not know that computer.",
        TailscaleError::Malformed => "Tailscale gave an answer that couldn't be read.",
        TailscaleError::TimedOut => "Tailscale did not answer in time.",
        _ => "Tailscale could not be asked.",
    }
}

/// Why a computer was not reached, as a plain sentence ending in a full stop.
pub fn fault(fault: &DialError) -> String {
    match fault {
        DialError::Tailscale(error) => tailscale(error).to_owned(),
        DialError::NoSuchComputer => {
            "That computer is not on your Tailscale network any more.".to_owned()
        }
        DialError::NotYours(_) => "That computer is not one of your own.".to_owned(),
        DialError::Offline => "That computer is switched off or out of reach.".to_owned(),
        DialError::Changed => {
            "That computer's place on the network now belongs to another computer.".to_owned()
        }
        DialError::Unreachable => "That computer did not answer.".to_owned(),
        _ => "That computer could not be reached.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_tailscale::MachineOwner;

    /// Words a person reads never include a program name or a term of the network's plumbing.
    const JARGON: [&str; 16] = [
        "porter", "inferd", "accountd", "socket", "port ", "node", "tailnet", "address",
        "localapi", "daemon", "whois", "http", "tcp", "listener", "tagged", "api",
    ];

    fn plain(text: &str) {
        assert!(text.ends_with('.'), "{text}");
        let lower = text.to_lowercase();
        for word in JARGON {
            assert!(!lower.contains(word), "{text:?} says {word:?}");
        }
    }

    #[test]
    fn every_refusal_reads_as_plain_words() {
        for it in Refusal::ALL {
            plain(refusal(&it));
        }
    }

    #[test]
    fn every_fault_reads_as_plain_words() {
        for it in [
            DialError::Tailscale(TailscaleError::NotRunning),
            DialError::Tailscale(TailscaleError::SignedOut),
            DialError::Tailscale(TailscaleError::Refused),
            DialError::NoSuchComputer,
            DialError::NotYours(MachineOwner::Shared),
            DialError::Offline,
            DialError::Changed,
            DialError::Unreachable,
        ] {
            plain(&fault(&it));
        }
    }

    #[test]
    fn tailscale_is_said_in_the_words_the_person_knows() {
        assert_eq!(
            fault(&DialError::Tailscale(TailscaleError::NotRunning)),
            "Tailscale isn't running on this computer."
        );
    }
}

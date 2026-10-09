//! Which group of the Accounts page a provider's accounts are listed under, and the group of the
//! add list: Internet Accounts (mail, calendar, files), Intelligence (AI services and models) and
//! Assistants (agent programs that sign themselves in). A provider file names its group
//! (`group = "internet" | "intelligence" | "agent"`); a file that does not gets the one
//! [`ProviderGroup::fallback`] decides from how its accounts sign in and what they do.

use crate::{AuthKind, CapabilityKind};
use serde::{Deserialize, Serialize};

/// The group a provider's accounts belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderGroup {
    /// Mail, calendar, contacts, files and the like.
    Internet,
    /// AI services and the models on this computer.
    Intelligence,
    /// Assistant programs that sign themselves in.
    Agent,
}

impl ProviderGroup {
    /// Every group, in the order a page lists them.
    pub const ALL: [ProviderGroup; 3] = [
        ProviderGroup::Internet,
        ProviderGroup::Intelligence,
        ProviderGroup::Agent,
    ];

    /// The stable word, as the file and the Settings row say it.
    pub fn slug(self) -> &'static str {
        match self {
            ProviderGroup::Internet => "internet",
            ProviderGroup::Intelligence => "intelligence",
            ProviderGroup::Agent => "agent",
        }
    }

    /// The group as a person reads it, the heading of its part of the page.
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderGroup::Internet => "Internet Accounts",
            ProviderGroup::Intelligence => "Intelligence",
            ProviderGroup::Agent => "Assistants",
        }
    }

    /// The group of a provider whose file does not name one, from the way its accounts sign in
    /// and the kinds of service they offer: an agent that signs itself in is an `Agent`; a
    /// runtime on this computer, and a provider whose services are all AI (an API key, a minted
    /// key, a plan; the coding agent a key provider also names counts as AI), is
    /// `Intelligence`; everything else is `Internet`.
    pub fn fallback(auth: AuthKind, kinds: &[CapabilityKind]) -> Self {
        let ai = |kind: &CapabilityKind| kind.is_ai() || *kind == CapabilityKind::Agent;
        let only_ai = !kinds.is_empty() && kinds.iter().all(ai);
        match auth {
            AuthKind::AgentLogin => ProviderGroup::Agent,
            AuthKind::LocalRuntime => ProviderGroup::Intelligence,
            _ if only_ai => ProviderGroup::Intelligence,
            _ => ProviderGroup::Internet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind::*;

    #[test]
    fn the_slug_is_the_serde_word_and_the_heading_is_plain() {
        let table = [
            (ProviderGroup::Internet, "internet", "Internet Accounts"),
            (ProviderGroup::Intelligence, "intelligence", "Intelligence"),
            (ProviderGroup::Agent, "agent", "Assistants"),
        ];
        for (group, slug, heading) in table {
            assert_eq!(group.slug(), slug);
            assert_eq!(group.display_name(), heading);
            assert_eq!(
                serde_json::to_string(&group).expect("json"),
                format!("\"{slug}\"")
            );
            let back: ProviderGroup = serde_json::from_str(&format!("\"{slug}\"")).expect("back");
            assert_eq!(back, group);
        }
        assert_eq!(ProviderGroup::ALL.len(), 3);
        assert!(serde_json::from_str::<ProviderGroup>("\"other\"").is_err());
    }

    #[test]
    fn a_file_without_a_group_gets_one_from_its_sign_in_and_its_services() {
        let table = [
            (AuthKind::AgentLogin, vec![Agent], ProviderGroup::Agent),
            (
                AuthKind::LocalRuntime,
                vec![Llm],
                ProviderGroup::Intelligence,
            ),
            (
                AuthKind::ApiKey,
                vec![Llm, Embeddings],
                ProviderGroup::Intelligence,
            ),
            (
                AuthKind::ApiKey,
                vec![Llm, Agent],
                ProviderGroup::Intelligence,
            ),
            (
                AuthKind::OAuthMintsKey,
                vec![Llm],
                ProviderGroup::Intelligence,
            ),
            (AuthKind::None, vec![Llm], ProviderGroup::Intelligence),
            (
                AuthKind::ApiKey,
                vec![Llm, Storage],
                ProviderGroup::Internet,
            ),
            (
                AuthKind::OAuthPkce,
                vec![Mail, Calendar],
                ProviderGroup::Internet,
            ),
            (AuthKind::Password, vec![Mail], ProviderGroup::Internet),
            (AuthKind::OwnProgram, vec![], ProviderGroup::Internet),
            (
                AuthKind::LoginFlowV2,
                vec![Storage],
                ProviderGroup::Internet,
            ),
            (AuthKind::ApiKey, vec![], ProviderGroup::Internet),
        ];
        for (auth, kinds, want) in table {
            assert_eq!(
                ProviderGroup::fallback(auth, &kinds),
                want,
                "{auth:?} {kinds:?}"
            );
        }
    }
}

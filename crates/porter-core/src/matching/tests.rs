use super::*;
use crate::capability::{
    Access, Albums, CapabilityKind, Delta, LibraryRead, LlmFeature, Offered, PhotosCap,
    QuotaReport, StorageScope,
};
use crate::capability::{AgentCap, AgentProgram, AgentProtocol, EnvName};
use crate::capability::{CuaBatching, CuaCap, CuaEnv, LlmWire};
use crate::fixtures::{llm, mail, storage};
use crate::need::AgentNeed;
use crate::need::{CuaNeed, LlmNeed, MailNeed, PhotosNeed, StorageNeed};
use crate::units::{Px, Tokens};

fn agent(program: &str, base_url_env: Option<&str>, protocols: &[AgentProtocol]) -> Offer {
    present(Capability::Agent(Box::new(AgentCap {
        program: AgentProgram::parse(program).expect("program"),
        key_env: Some(EnvName::parse("SOME_API_KEY").expect("env")),
        base_url_env: base_url_env.map(|name| EnvName::parse(name).expect("env")),
        protocols: protocols.iter().copied().collect(),
    })))
}

fn agent_need(program: &str, protocols: &[AgentProtocol], base_url: Offered) -> Need {
    Need::Agent(AgentNeed {
        program: AgentProgram::parse(program).expect("program"),
        protocols: protocols.iter().copied().collect(),
        base_url,
    })
}

fn storage_need(access: Access, delta: Delta, scope: StorageScope) -> Need {
    Need::Storage(StorageNeed {
        access,
        delta,
        scope,
        quota: QuotaReport::Unreported,
    })
}

fn present(capability: Capability) -> Offer {
    Offer::Present(capability)
}

fn computer_use(environments: &[CuaEnv]) -> Offer {
    present(Capability::ComputerUse(CuaCap {
        environments: environments.iter().copied().collect(),
        batching: CuaBatching::Many,
        zoom: Offered::Absent,
        max_image: Px(1568),
        wire: LlmWire::ChatCompletions,
    }))
}

fn cua_need(environments: &[CuaEnv]) -> Need {
    Need::ComputerUse(CuaNeed {
        environments: environments.iter().copied().collect(),
    })
}

fn google_photos() -> Offer {
    present(Capability::Photos(PhotosCap {
        library_read: LibraryRead::PickerOnly,
        upload: Offered::Present,
        albums: Albums::AppCreated,
        video: Offered::Present,
        delta: Delta::None,
    }))
}

fn cases() -> Vec<(&'static str, Need, Offer, Match)> {
    use Access::{Read, ReadWrite};
    use StorageScope::{AppFolder, Full};
    let chat_tools: &[LlmFeature] = &[LlmFeature::Chat, LlmFeature::Tools];
    vec![
        (
            "storage write+poll fits a push store",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::Push, Full)),
            Match::Fits,
        ),
        (
            "equal minimums fit",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::Poll, AppFolder)),
            Match::Fits,
        ),
        (
            "read-only store falls short on access",
            storage_need(ReadWrite, Delta::Poll, AppFolder),
            present(storage(Read, Delta::Push, Full)),
            Match::Short(Shortfall::Access),
        ),
        (
            "no-delta store falls short on delta",
            storage_need(Read, Delta::Poll, AppFolder),
            present(storage(ReadWrite, Delta::None, Full)),
            Match::Short(Shortfall::Delta),
        ),
        (
            "app folder falls short of a full-store need",
            storage_need(Read, Delta::None, Full),
            present(storage(ReadWrite, Delta::Push, AppFolder)),
            Match::Short(Shortfall::Scope),
        ),
        (
            "first shortfall wins",
            storage_need(ReadWrite, Delta::Push, Full),
            present(storage(Read, Delta::None, AppFolder)),
            Match::Short(Shortfall::Access),
        ),
        (
            "absent storage says why",
            storage_need(Read, Delta::None, AppFolder),
            Offer::Absent {
                kind: CapabilityKind::Storage,
                reason: AbsentReason::ProviderOffersNone,
            },
            Match::Absent(AbsentReason::ProviderOffersNone),
        ),
        (
            "absent of another kind is another kind",
            storage_need(Read, Delta::None, AppFolder),
            Offer::Absent {
                kind: CapabilityKind::Notes,
                reason: AbsentReason::TenantConsent,
            },
            Match::OtherKind,
        ),
        (
            "mail offer does not answer a storage need",
            storage_need(Read, Delta::None, AppFolder),
            present(mail(Offered::Present)),
            Match::OtherKind,
        ),
        (
            "mail without send falls short on send",
            Need::Mail(MailNeed {
                access: Read,
                send: Offered::Present,
                delta: Delta::None,
            }),
            present(mail(Offered::Absent)),
            Match::Short(Shortfall::Send),
        ),
        (
            "google photos cannot mirror a library",
            Need::Photos(PhotosNeed {
                library_read: LibraryRead::Full,
                upload: Offered::Absent,
                albums: Albums::None,
                video: Offered::Absent,
                delta: Delta::None,
            }),
            google_photos(),
            Match::Short(Shortfall::LibraryRead),
        ),
        (
            "google photos is an upload target",
            Need::Photos(PhotosNeed {
                library_read: LibraryRead::None,
                upload: Offered::Present,
                albums: Albums::AppCreated,
                video: Offered::Absent,
                delta: Delta::None,
            }),
            google_photos(),
            Match::Fits,
        ),
        (
            "llm with the features and window fits",
            Need::Llm(LlmNeed {
                features: chat_tools.iter().copied().collect(),
                context: Tokens(32_000),
            }),
            present(llm(
                &[LlmFeature::Chat, LlmFeature::Tools, LlmFeature::Vision],
                128_000,
            )),
            Match::Fits,
        ),
        (
            "llm without tools falls short on features",
            Need::Llm(LlmNeed {
                features: chat_tools.iter().copied().collect(),
                context: Tokens(8_000),
            }),
            present(llm(&[LlmFeature::Chat], 128_000)),
            Match::Short(Shortfall::Features),
        ),
        (
            "llm with a small window falls short on context",
            Need::Llm(LlmNeed {
                features: Default::default(),
                context: Tokens(32_000),
            }),
            present(llm(&[LlmFeature::Chat], 8_000)),
            Match::Short(Shortfall::Context),
        ),
        (
            "the agent asked for fits the agent offered",
            agent_need("claude-code", &[], Offered::Absent),
            agent("claude-code", None, &[AgentProtocol::AnthropicMessages]),
            Match::Fits,
        ),
        (
            "another agent program is not the one asked for",
            agent_need("codex", &[], Offered::Absent),
            agent("claude-code", Some("ANTHROPIC_BASE_URL"), &[]),
            Match::Short(Shortfall::Program),
        ),
        (
            "an agent that speaks only Messages falls short of an OpenAI-compatible need",
            agent_need(
                "claude-code",
                &[AgentProtocol::OpenAiCompatible],
                Offered::Absent,
            ),
            agent("claude-code", None, &[AgentProtocol::AnthropicMessages]),
            Match::Short(Shortfall::Protocols),
        ),
        (
            "an agent with no base-url variable falls short of a routed need",
            agent_need("gemini-cli", &[], Offered::Present),
            agent("gemini-cli", None, &[AgentProtocol::OpenAiCompatible]),
            Match::Short(Shortfall::BaseUrl),
        ),
        (
            "an agent with a base-url variable fits a routed need",
            agent_need(
                "codex",
                &[AgentProtocol::OpenAiCompatible],
                Offered::Present,
            ),
            agent(
                "codex",
                Some("OPENAI_BASE_URL"),
                &[AgentProtocol::OpenAiCompatible],
            ),
            Match::Fits,
        ),
        (
            "an llm offer is another kind for an agent need",
            agent_need("codex", &[], Offered::Absent),
            present(llm(&[LlmFeature::Chat], 8_000)),
            Match::OtherKind,
        ),
        (
            "computer use on the desktop fits a desktop model",
            cua_need(&[CuaEnv::Desktop]),
            computer_use(&[CuaEnv::Desktop, CuaEnv::Browser]),
            Match::Fits,
        ),
        (
            "computer use on a phone falls short on environments",
            cua_need(&[CuaEnv::Desktop, CuaEnv::Mobile]),
            computer_use(&[CuaEnv::Desktop]),
            Match::Short(Shortfall::Environments),
        ),
        (
            "a computer-use need is not met by a chat model",
            cua_need(&[CuaEnv::Desktop]),
            present(llm(&[LlmFeature::Vision], 8_000)),
            Match::OtherKind,
        ),
    ]
}

#[test]
fn matches_answers_each_need_against_each_offer() {
    for (name, need, offer, expected) in cases() {
        assert_eq!(matches(&need, &offer), expected, "{name}");
    }
}

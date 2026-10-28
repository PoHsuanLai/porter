//! A need and a candidate survive their D-Bus shape, and the shape is what the introspection
//! says: a kind slug and a vardict for a need, a path, a label and a vardict for a candidate.

use porter_capability::ModelId;
use porter_capability::Subject;
use porter_capability::{
    Access, AgentCap, AgentProgram, AgentProtocol, Albums, Capability, CapabilityKind, CuaEnv,
    Delta, HashKind, LibraryRead, LlmCap, LlmFeature, LlmWire, Modality, Offered, QuotaReport,
    SpeechMode, StorageCap, StorageScope,
};
use porter_core::need::{
    AgentNeed, CuaNeed, DimsNeed, EmbedNeed, IdentityNeed, ImageGenNeed, KeyValueNeed, LlmNeed,
    MailNeed, NotesNeed, PhotosNeed, PimNeed, PushNeed, RerankNeed, SpeechNeed, StorageNeed,
};
use porter_core::{AccountId, GrantId};
use porter_core::{
    Candidate, EndpointUrl, Family, Limit, LimitReason, LoginName, Need, Restriction,
    ServiceEndpoint, TenantConsent, Tls, TokenLifetime, Verification,
};
use porter_dbus::names::account_path;
use porter_dbus::{candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus};
use porter_ids::{Bytes, Count, Dims, Px, Tokens};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

fn every_need() -> Vec<(&'static str, Need)> {
    let pim = PimNeed::new(Access::Read, Delta::Poll);
    vec![
        (
            "identity",
            Need::Identity(IdentityNeed::new(Offered::Present, Offered::Absent)),
        ),
        (
            "mail",
            Need::Mail(MailNeed::new(Access::Read, Offered::Present, Delta::Push)),
        ),
        ("calendar", Need::Calendar(pim.clone())),
        ("contacts", Need::Contacts(pim.clone())),
        ("tasks", Need::Tasks(pim)),
        (
            "notes",
            Need::Notes(NotesNeed::new(Access::ReadWrite, Delta::None)),
        ),
        (
            "storage",
            Need::Storage(StorageNeed::new(
                Access::ReadWrite,
                Delta::Poll,
                StorageScope::AppFolder,
                QuotaReport::Unreported,
            )),
        ),
        (
            "photos",
            Need::Photos(PhotosNeed::new(
                LibraryRead::None,
                Offered::Present,
                Albums::None,
                Offered::Absent,
                Delta::None,
            )),
        ),
        (
            "llm",
            Need::Llm(LlmNeed::new(
                [LlmFeature::Vision, LlmFeature::Tools].into(),
                Tokens(32_000),
            )),
        ),
        (
            "embeddings",
            Need::Embeddings(EmbedNeed::new(
                DimsNeed::Exactly(Dims(768)),
                [Modality::Text, Modality::Image].into(),
            )),
        ),
        (
            "embeddings",
            Need::Embeddings(EmbedNeed::new(DimsNeed::Any, [Modality::Text].into())),
        ),
        (
            "speech",
            Need::Speech(SpeechNeed::new([SpeechMode::Stt, SpeechMode::Tts].into())),
        ),
        (
            "image_gen",
            Need::ImageGen(ImageGenNeed::new(
                [porter_capability::ImageMode::Inpaint].into(),
                Px(512),
            )),
        ),
        ("rerank", Need::Rerank(RerankNeed::new(Count(10)))),
        (
            "computer_use",
            Need::ComputerUse(CuaNeed::new([CuaEnv::Desktop, CuaEnv::Browser].into())),
        ),
        (
            "key_value",
            Need::KeyValue(KeyValueNeed::new(Delta::Poll, Bytes(1024))),
        ),
        ("push", Need::Push(PushNeed::new())),
        (
            "agent",
            Need::Agent(AgentNeed::new(
                AgentProgram::parse("claude-code").expect("program"),
                [AgentProtocol::AnthropicMessages].into(),
                Offered::Present,
            )),
        ),
    ]
}

#[test]
fn every_need_round_trips_under_its_kind_slug() {
    for (slug, need) in every_need() {
        let (kind, details) = need_to_dbus(&need);
        assert_eq!(kind, slug, "{need:?}");
        assert_eq!(
            need_from_dbus((kind, details)),
            Ok(need.clone()),
            "{need:?}"
        );
    }
}

#[test]
fn a_needs_fields_are_entries_by_name() {
    let need = Need::Llm(LlmNeed::new([LlmFeature::Vision].into(), Tokens(32_000)));
    let (_, details) = need_to_dbus(&need);
    let mut names: Vec<&str> = details.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["context", "features"]);
    let context: &Value<'_> = &details["context"];
    assert_eq!(*context, Value::I64(32_000));
}

#[test]
fn a_need_survives_the_bus_signature() {
    // Through the real wire encoding, not just the in-memory value: `(sa{sv})`.
    let ctxt = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
    for (_, need) in every_need() {
        let arg = need_to_dbus(&need);
        let bytes = zbus::zvariant::to_bytes(ctxt, &arg).expect("encodes");
        let (back, _): (porter_dbus::NeedArg, usize) = bytes.deserialize().expect("decodes");
        assert_eq!(need_from_dbus(back), Ok(need));
    }
}

#[test]
fn a_malformed_need_is_refused_not_guessed() {
    let (kind, mut details) = need_to_dbus(&every_need()[8].1);
    details.insert(
        "context".to_owned(),
        OwnedValue::try_from(Value::from("lots")).expect("value"),
    );
    assert!(need_from_dbus((kind.clone(), details.clone())).is_err());
    details.remove("context");
    assert!(need_from_dbus((kind, details)).is_err(), "a missing field");
}

fn candidates() -> Vec<Candidate> {
    let storage = Capability::Storage(StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: QuotaReport::Reported,
        scope: StorageScope::AppFolder,
        hashes: HashKind::QuickXor,
        ranges: Offered::Present,
        chunked_upload: Offered::Present,
    });
    let llm = Capability::Llm(LlmCap {
        features: [LlmFeature::Chat, LlmFeature::Tools].into(),
        context: Tokens(128_000),
        max_output: Tokens(8_192),
        wire: LlmWire::Messages,
    });
    vec![
        Candidate::new(
            AccountId::parse("67e55044-10b1.x").expect("id"),
            porter_core::AccountLabel("ada@example.org".into()),
            "nextcloud".parse_provider(),
            Subject::Account,
            storage,
            Restriction::none(),
            GrantId::parse("g1").expect("grant"),
        )
        .with_endpoints(vec![
            endpoint(
                Family::WebDav,
                "https://cloud.example.org/remote.php/dav/",
                Tls::Implicit,
            ),
            endpoint(Family::Imap, "imap://imap.example.org:143", Tls::StartTls),
        ]),
        Candidate::new(
            AccountId::parse("local-ollama").expect("id"),
            porter_core::AccountLabel("Ollama on this computer".into()),
            "ollama".parse_provider(),
            Subject::Model(ModelId::parse("llama3.2").expect("model")),
            llm,
            Restriction::none()
                .with_verification(Verification::Unverified {
                    user_cap: Count(100),
                })
                .with_token_lifetime(TokenLifetime::SevenDays)
                .with_consent(TenantConsent::AdminRequired)
                .with_limits(vec![Limit::new(
                    CapabilityKind::Llm,
                    LimitReason::PickerOnly,
                )]),
            GrantId::parse("g2").expect("grant"),
        ),
    ]
}

#[test]
fn an_agent_candidate_with_no_variable_to_set_round_trips() {
    // An absent variable is an absent entry, not a null: the vardict has no null.
    let candidate = Candidate::new(
        AccountId::parse("gemini-cli").expect("id"),
        porter_core::AccountLabel("Gemini CLI".into()),
        "gemini-cli".parse_provider(),
        Subject::Agent(AgentProgram::parse("gemini-cli").expect("program")),
        Capability::Agent(Box::new(AgentCap {
            program: AgentProgram::parse("gemini-cli").expect("program"),
            key_env: Some(porter_capability::EnvName::parse("GEMINI_API_KEY").expect("env")),
            base_url_env: None,
            protocols: [AgentProtocol::GenerateContent].into(),
        })),
        Restriction::none(),
        GrantId::parse("g3").expect("grant"),
    );
    let ctxt = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
    let arg = candidate_to_dbus(&candidate);
    let bytes = zbus::zvariant::to_bytes(ctxt, &arg).expect("encodes");
    let (back, _): (porter_dbus::CandidateArg, usize) = bytes.deserialize().expect("decodes");
    assert_eq!(candidate_from_dbus(back), Ok(candidate));
}

fn endpoint(family: Family, url: &str, tls: Tls) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: EndpointUrl::parse(url).expect("url"),
        tls,
        login: LoginName("ada".into()),
    }
}

trait ParseProvider {
    fn parse_provider(&self) -> porter_core::ProviderId;
}

impl ParseProvider for str {
    fn parse_provider(&self) -> porter_core::ProviderId {
        porter_core::ProviderId::parse(self).expect("provider id")
    }
}

#[test]
fn a_candidate_round_trips_through_its_shape() {
    for candidate in candidates() {
        let arg = candidate_to_dbus(&candidate);
        assert_eq!(arg.0.as_str(), account_path(&candidate.account));
        assert_eq!(arg.1, candidate.label.0);
        assert_eq!(candidate_from_dbus(arg), Ok(candidate.clone()));
    }
}

#[test]
fn a_candidate_survives_the_bus_signature() {
    let ctxt = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
    for candidate in candidates() {
        let arg = candidate_to_dbus(&candidate);
        let bytes = zbus::zvariant::to_bytes(ctxt, &arg).expect("encodes");
        let (back, _): (porter_dbus::CandidateArg, usize) = bytes.deserialize().expect("decodes");
        assert_eq!(candidate_from_dbus(back), Ok(candidate));
    }
}

#[test]
fn a_candidate_whose_path_is_another_account_is_refused() {
    let [first, second] = <[Candidate; 2]>::try_from(candidates()).expect("two");
    let (_, label, details) = candidate_to_dbus(&first);
    let wrong = OwnedObjectPath::try_from(account_path(&second.account)).expect("path");
    assert!(candidate_from_dbus((wrong, label, details)).is_err());
}

#[test]
fn the_kind_slug_is_the_capability_kind_slug() {
    for (_, need) in every_need() {
        let slug = serde_json::to_value(need.kind())
            .expect("json")
            .as_str()
            .map(str::to_owned);
        assert_eq!(slug, Some(need_to_dbus(&need).0));
    }
}

#[test]
fn a_candidate_carries_its_endpoints_in_the_endpoints_key() {
    let [with, without] = <[Candidate; 2]>::try_from(candidates()).expect("two");
    let arg = candidate_to_dbus(&with);
    assert!(arg.2.contains_key("endpoints"), "{:?}", arg.2.keys());
    assert_eq!(candidate_from_dbus(arg), Ok(with));
    let arg = candidate_to_dbus(&without);
    assert!(
        arg.2.contains_key("endpoints"),
        "an empty list is still written"
    );
    assert_eq!(candidate_from_dbus(arg), Ok(without));
}

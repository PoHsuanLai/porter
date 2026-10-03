//! A need and a candidate survive their D-Bus shape, and the shape is what the introspection
//! says: a kind slug and a vardict for a need, a path, a label and a vardict for a candidate.

use porter_core::capability::{
    Access, Albums, Capability, CapabilityKind, CuaEnv, Delta, HashKind, LibraryRead, LlmCap,
    LlmFeature, LlmWire, Modality, Offered, QuotaReport, SpeechMode, StorageCap, StorageScope,
};
use porter_core::need::{
    CuaNeed, DimsNeed, EmbedNeed, IdentityNeed, ImageGenNeed, KeyValueNeed, LlmNeed, MailNeed,
    NotesNeed, PhotosNeed, PimNeed, PushNeed, RerankNeed, SpeechNeed, StorageNeed,
};
use porter_core::{
    AccountId, Bytes, Candidate, Count, Dims, GrantId, Limit, LimitReason, ModelId, Need, Px,
    Restriction, Subject, TenantConsent, TokenLifetime, Tokens, Verification,
};
use porter_dbus::{
    account_path, candidate_from_dbus, candidate_to_dbus, need_from_dbus, need_to_dbus,
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

fn every_need() -> Vec<(&'static str, Need)> {
    let pim = PimNeed {
        access: Access::Read,
        delta: Delta::Poll,
    };
    vec![
        (
            "identity",
            Need::Identity(IdentityNeed {
                profile: Offered::Present,
                verified_address: Offered::Absent,
            }),
        ),
        (
            "mail",
            Need::Mail(MailNeed {
                access: Access::Read,
                send: Offered::Present,
                delta: Delta::Push,
            }),
        ),
        ("calendar", Need::Calendar(pim.clone())),
        ("contacts", Need::Contacts(pim.clone())),
        ("tasks", Need::Tasks(pim)),
        (
            "notes",
            Need::Notes(NotesNeed {
                access: Access::ReadWrite,
                delta: Delta::None,
            }),
        ),
        (
            "storage",
            Need::Storage(StorageNeed {
                access: Access::ReadWrite,
                delta: Delta::Poll,
                scope: StorageScope::AppFolder,
                quota: QuotaReport::Unreported,
            }),
        ),
        (
            "photos",
            Need::Photos(PhotosNeed {
                library_read: LibraryRead::None,
                upload: Offered::Present,
                albums: Albums::None,
                video: Offered::Absent,
                delta: Delta::None,
            }),
        ),
        (
            "llm",
            Need::Llm(LlmNeed {
                features: [LlmFeature::Vision, LlmFeature::Tools].into(),
                context: Tokens(32_000),
            }),
        ),
        (
            "embeddings",
            Need::Embeddings(EmbedNeed {
                dims: DimsNeed::Exactly(Dims(768)),
                modalities: [Modality::Text, Modality::Image].into(),
            }),
        ),
        (
            "embeddings",
            Need::Embeddings(EmbedNeed {
                dims: DimsNeed::Any,
                modalities: [Modality::Text].into(),
            }),
        ),
        (
            "speech",
            Need::Speech(SpeechNeed {
                modes: [SpeechMode::Stt, SpeechMode::Tts].into(),
            }),
        ),
        (
            "image_gen",
            Need::ImageGen(ImageGenNeed {
                modes: [porter_core::capability::ImageMode::Inpaint].into(),
                max_side: Px(512),
            }),
        ),
        (
            "rerank",
            Need::Rerank(RerankNeed {
                max_docs: Count(10),
            }),
        ),
        (
            "computer_use",
            Need::ComputerUse(CuaNeed {
                environments: [CuaEnv::Desktop, CuaEnv::Browser].into(),
            }),
        ),
        (
            "key_value",
            Need::KeyValue(KeyValueNeed {
                delta: Delta::Poll,
                max_item: Bytes(1024),
            }),
        ),
        ("push", Need::Push(PushNeed {})),
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
    let need = Need::Llm(LlmNeed {
        features: [LlmFeature::Vision].into(),
        context: Tokens(32_000),
    });
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
        Candidate {
            account: AccountId::parse("67e55044-10b1.x").expect("id"),
            label: porter_core::AccountLabel("ada@example.org".into()),
            provider: "nextcloud".parse_provider(),
            subject: Subject::Account,
            capability: storage,
            restriction: Restriction::none(),
            grant: GrantId::parse("g1").expect("grant"),
        },
        Candidate {
            account: AccountId::parse("local-ollama").expect("id"),
            label: porter_core::AccountLabel("Ollama on this computer".into()),
            provider: "ollama".parse_provider(),
            subject: Subject::Model(ModelId::parse("llama3.2").expect("model")),
            capability: llm,
            restriction: Restriction {
                verification: Verification::Unverified {
                    user_cap: Count(100),
                },
                token_lifetime: TokenLifetime::SevenDays,
                consent: TenantConsent::AdminRequired,
                limits: vec![Limit {
                    kind: CapabilityKind::Llm,
                    reason: LimitReason::PickerOnly,
                }],
            },
            grant: GrantId::parse("g2").expect("grant"),
        },
    ]
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

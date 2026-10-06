//! Every stored or wire type of porter-core survives its serde form, and the forms that other
//! programs read keep their slugs.

use porter_core::capability::{
    Access, Albums, Capability, CapabilityKind, CuaBatching, CuaCap, CuaEnv, Delta, EmbedCap,
    EmbedPrompts, HashKind, IdentityCap, ImageGenCap, ImageMode, KeyValueCap, LabelModel,
    LanguageSet, LanguageTag, LibraryRead, LlmCap, LlmFeature, LlmWire, MailCap, MailTransport,
    Modality, NotesCap, NotesTransport, Offered, PhotosCap, PimCap, PimTransport, PrefixText,
    PushCap, PushChannel, QuotaReport, RerankCap, SpeechCap, SpeechMode, StorageCap, StorageScope,
    VocabVersion,
};
use porter_core::consent::{
    AccountChoice, Availability, ConsentAnswer, ConsentAsk, Decision, Grant, GrantKey, GrantScope,
    Usage, Verdict,
};
use porter_core::need::{
    CuaNeed, DimsNeed, EmbedNeed, IdentityNeed, ImageGenNeed, KeyValueNeed, LlmNeed, MailNeed,
    NotesNeed, PhotosNeed, PimNeed, PushNeed, RerankNeed, SpeechNeed, StorageNeed,
};
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldProblem, FieldSpec, FieldValue, Presence, ProblemKind,
    Progress, ProviderRow, Review, ReviewView, RowKind, ServiceChoice, ServiceRow, ServiceState,
    SheetInput, SheetView, SignInFault, SignInView, UserCode,
};
use porter_core::store::{AccountToggle, Persisted};
use porter_core::wire::{LegacyItem, LegacyRef, ParentWindow, ProviderHint, Refusal};
use porter_core::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value, "{json}");
}

fn account_id(s: &str) -> AccountId {
    AccountId::parse(s).expect("account id")
}
fn grant_id(s: &str) -> GrantId {
    GrantId::parse(s).expect("grant id")
}
fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn every_capability() -> Vec<Capability> {
    let pim = PimCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        transport: PimTransport::CalDav,
        collections: Offered::Present,
    };
    vec![
        Capability::Identity(IdentityCap {
            profile: Offered::Present,
            verified_address: Offered::Present,
            sign_in: Offered::Absent,
        }),
        Capability::Mail(MailCap {
            access: Access::ReadWrite,
            send: Offered::Present,
            delta: Delta::Push,
            transport: MailTransport::Jmap,
            labels: LabelModel::Labels,
        }),
        Capability::Calendar(pim.clone()),
        Capability::Contacts(PimCap {
            transport: PimTransport::CardDav,
            ..pim.clone()
        }),
        Capability::Tasks(PimCap {
            transport: PimTransport::GoogleApi,
            ..pim
        }),
        Capability::Notes(NotesCap {
            access: Access::Read,
            delta: Delta::None,
            transport: NotesTransport::OneNote,
        }),
        Capability::Storage(StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::QuickXor,
            ranges: Offered::Present,
            chunked_upload: Offered::Present,
        }),
        Capability::Photos(PhotosCap {
            library_read: LibraryRead::PickerOnly,
            upload: Offered::Present,
            albums: Albums::AppCreated,
            video: Offered::Present,
            delta: Delta::None,
        }),
        Capability::Llm(LlmCap {
            features: [LlmFeature::Chat, LlmFeature::Tools].into(),
            context: Tokens(128_000),
            max_output: Tokens(8_192),
            wire: LlmWire::Messages,
        }),
        Capability::Embeddings(EmbedCap {
            dims: Dims(768),
            modalities: [Modality::Text].into(),
            max_input: Tokens(8_192),
            max_batch: Count(32),
            prompts: Box::new(EmbedPrompts {
                query: PrefixText("search_query: ".into()),
                document: PrefixText("search_document: ".into()),
            }),
        }),
        Capability::Speech(SpeechCap {
            modes: [SpeechMode::Stt, SpeechMode::Tts].into(),
            languages: LanguageSet::Listed([LanguageTag::parse("zh-Hant-TW").expect("tag")].into()),
        }),
        Capability::ImageGen(ImageGenCap {
            modes: [ImageMode::TextToImage].into(),
            max_side: Px(1024),
        }),
        Capability::Rerank(RerankCap {
            max_docs: Count(100),
        }),
        Capability::ComputerUse(CuaCap {
            environments: [CuaEnv::Desktop, CuaEnv::Browser].into(),
            batching: CuaBatching::Many,
            zoom: Offered::Present,
            max_image: Px(1568),
            wire: LlmWire::ChatCompletions,
        }),
        Capability::KeyValue(KeyValueCap {
            delta: Delta::Poll,
            max_item: Bytes(65_536),
        }),
        Capability::Push(PushCap {
            channel: PushChannel::WebSocket,
        }),
    ]
}

fn every_need() -> Vec<Need> {
    let pim = PimNeed {
        access: Access::Read,
        delta: Delta::Poll,
    };
    vec![
        Need::Identity(IdentityNeed {
            profile: Offered::Present,
            verified_address: Offered::Absent,
        }),
        Need::Mail(MailNeed {
            access: Access::Read,
            send: Offered::Present,
            delta: Delta::Push,
        }),
        Need::Calendar(pim.clone()),
        Need::Contacts(pim.clone()),
        Need::Tasks(pim),
        Need::Notes(NotesNeed {
            access: Access::ReadWrite,
            delta: Delta::None,
        }),
        Need::Storage(StorageNeed {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            scope: StorageScope::AppFolder,
            quota: QuotaReport::Unreported,
        }),
        Need::Photos(PhotosNeed {
            library_read: LibraryRead::None,
            upload: Offered::Present,
            albums: Albums::None,
            video: Offered::Absent,
            delta: Delta::None,
        }),
        Need::Llm(LlmNeed {
            features: [LlmFeature::Vision].into(),
            context: Tokens(32_000),
        }),
        Need::Embeddings(EmbedNeed {
            dims: DimsNeed::Exactly(Dims(768)),
            modalities: [Modality::Image].into(),
        }),
        Need::Speech(SpeechNeed {
            modes: [SpeechMode::Realtime].into(),
        }),
        Need::ImageGen(ImageGenNeed {
            modes: [ImageMode::Inpaint].into(),
            max_side: Px(512),
        }),
        Need::Rerank(RerankNeed {
            max_docs: Count(10),
        }),
        Need::ComputerUse(CuaNeed {
            environments: [CuaEnv::Desktop].into(),
        }),
        Need::KeyValue(KeyValueNeed {
            delta: Delta::Poll,
            max_item: Bytes(1024),
        }),
        Need::Push(PushNeed {}),
    ]
}

fn endpoints() -> Vec<ServiceEndpoint> {
    let url = |text: &str| EndpointUrl::parse(text).expect("url");
    vec![
        ServiceEndpoint {
            family: Family::Imap,
            url: url("imaps://imap.example.org"),
            tls: Tls::Implicit,
            login: LoginName("ada@example.org".into()),
        },
        ServiceEndpoint {
            family: Family::Smtp,
            url: url("smtp://smtp.example.org:587"),
            tls: Tls::StartTls,
            login: LoginName("ada".into()),
        },
        ServiceEndpoint {
            family: Family::WebDav,
            url: url("https://cloud.example.org/remote.php/dav/"),
            tls: Tls::Implicit,
            login: LoginName("ada".into()),
        },
        ServiceEndpoint {
            family: Family::Sieve,
            url: url("sieve://imap.example.org:4190"),
            tls: Tls::StartTls,
            login: LoginName("ada@example.org".into()),
        },
    ]
}

fn legacy_ref() -> LegacyRef {
    LegacyRef {
        account: account_id("67e55044-10b1-426f-9247-bb680e5fe0c8"),
        provider: ProviderId::parse("generic-imap").expect("provider"),
        label: AccountLabel("ada@example.org".into()),
        endpoints: endpoints()[..2].to_vec(),
        items: vec![
            LegacyItem::Incoming,
            LegacyItem::Outgoing,
            LegacyItem::OAuth,
            LegacyItem::AddressBook,
        ],
    }
}

fn restriction() -> Restriction {
    Restriction {
        verification: Verification::Unverified {
            user_cap: Count(100),
        },
        token_lifetime: TokenLifetime::SevenDays,
        consent: TenantConsent::AdminRequired,
        limits: vec![Limit {
            kind: CapabilityKind::Photos,
            reason: LimitReason::PickerOnly,
        }],
    }
}

fn candidate() -> Candidate {
    Candidate {
        account: account_id("cloud"),
        label: AccountLabel("ada@example.org".into()),
        provider: ProviderId::parse("nextcloud").expect("provider"),
        subject: Subject::Account,
        capability: every_capability()[6].clone(),
        restriction: restriction(),
        grant: grant_id("g1"),
        endpoints: endpoints(),
    }
}

fn grant() -> Grant {
    Grant {
        id: grant_id("g1"),
        key: GrantKey {
            app: app(),
            account: account_id("cloud"),
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Background,
            space: SpaceScope::Only(SpaceId::parse("work").expect("space")),
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1_790_000_000),
    }
}

#[test]
fn capabilities_needs_and_offers_round_trip() {
    every_capability().iter().for_each(round_trip);
    every_need().iter().for_each(round_trip);
    for capability in every_capability() {
        round_trip(&capability.kind());
        round_trip(&Claim {
            subject: Subject::Account,
            offer: Offer::Present(capability),
            provenance: Provenance::Probed,
        });
    }
    round_trip(&Claim {
        subject: Subject::Model(ModelId::parse("llama3.2").expect("model")),
        offer: Offer::Absent {
            kind: CapabilityKind::Notes,
            reason: AbsentReason::TenantConsent,
        },
        provenance: Provenance::Curated,
    });
    round_trip(&Match::Short(Shortfall::Scope));
    round_trip(&KindToggle {
        kind: CapabilityKind::Mail,
        toggle: Toggle::Off,
    });
}

#[test]
fn accounts_and_ai_properties_round_trip() {
    round_trip(&Account {
        id: account_id("cloud"),
        provider: ProviderId::parse("google").expect("provider"),
        label: AccountLabel("ada@example.org".into()),
        state: AccountState::Limited,
        auth: AuthKind::OAuthPkce,
        capabilities: vec![],
        restriction: restriction(),
        endpoints: endpoints(),
    });
    round_trip(&Locality::Cloud {
        region: Some(Region("eu-west-1".into())),
    });
    round_trip(&Locality::OnDevice);
    round_trip(&Tier::Balanced);
    round_trip(&Billing::Metered(PriceTable {
        input_per_mtok: MicroUsd(3_000_000),
        output_per_mtok: MicroUsd(15_000_000),
    }));
    round_trip(&Billing::PlanBudget);
}

#[test]
fn consent_values_round_trip() {
    round_trip(&grant());
    round_trip(&Verdict::Granted {
        grant: grant_id("g1"),
        scope: GrantScope::Once,
    });
    round_trip(&Availability::AvailableNeedsConsent);
    round_trip(&ConsentAsk {
        app: app(),
        kind: CapabilityKind::Storage,
        class: DataClass::Photos,
        usage: Usage::Interactive,
        accounts: vec![AccountChoice {
            account: account_id("cloud"),
            label: AccountLabel("Nextcloud".into()),
            provider: ProviderId::parse("nextcloud").expect("provider"),
        }],
    });
    round_trip(&ConsentAnswer::Allow {
        account: account_id("cloud"),
        scope: GrantScope::Always,
    });
    round_trip(&ConsentAnswer::Dismissed);
    round_trip(&ConsentAnswer::AddAccount);
    assert_eq!(
        serde_json::to_string(&ConsentAnswer::AddAccount).expect("json"),
        r#"{"kind":"add_account"}"#
    );
}

#[test]
fn secrets_and_tokens_round_trip() {
    for purpose in [
        SecretPurpose::Password,
        SecretPurpose::IncomingPassword,
        SecretPurpose::OutgoingPassword,
        SecretPurpose::ServicePassword(CapabilityKind::Contacts),
        SecretPurpose::OAuthRefresh,
        SecretPurpose::ApiKey,
        SecretPurpose::KeyPair,
        SecretPurpose::DeviceKey,
    ] {
        round_trip(&SecretKey {
            account: account_id("cloud"),
            purpose,
        });
    }
    round_trip(&Credential::OAuth {
        access: SecretText::new("a"),
        refresh: SecretText::new("r"),
        expires_at: UnixSeconds(5),
    });
    round_trip(&Credential::KeyPair {
        access_key: "AKIA".into(),
        secret: SecretText::new("s"),
    });
    round_trip(&IssuedToken {
        kind: TokenKind::Xoauth2,
        value: SecretText::new("t"),
        expires: UnixSeconds(9),
    });
}

#[test]
fn every_request_and_reply_round_trips() {
    let need = every_need()[6].clone();
    let window = ParentWindow::Handle("wayland:abc".into());
    let requests = vec![
        AccountsRequest::Query {
            need: need.clone(),
            class: DataClass::Photos,
            usage: Usage::Interactive,
        },
        AccountsRequest::Availability {
            need: need.clone(),
            class: DataClass::Files,
            usage: Usage::Background,
        },
        AccountsRequest::Choose {
            need,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            window: window.clone(),
        },
        AccountsRequest::AddAccount {
            hint: ProviderHint::Provider(ProviderId::parse("nextcloud").expect("id")),
            window: ParentWindow::Unparented,
        },
        AccountsRequest::AddAccount {
            hint: ProviderHint::Any,
            window: window.clone(),
        },
        AccountsRequest::Reauthenticate {
            account: account_id("cloud"),
            window,
        },
        AccountsRequest::ListGrants,
        AccountsRequest::Revoke {
            grant: grant_id("g1"),
        },
        AccountsRequest::IssueToken {
            grant: grant_id("g1"),
            audience: Audience("imap".into()),
        },
        AccountsRequest::OpenAuthenticated {
            grant: grant_id("g1"),
            endpoint: endpoints()[0].url.clone(),
        },
        AccountsRequest::OpenLinked {
            grant: grant_id("g1"),
            origin: endpoints()[0].url.clone(),
        },
        AccountsRequest::Adopt {
            legacy: legacy_ref(),
        },
    ];
    requests.iter().for_each(round_trip);
    let replies = vec![
        AccountsReply::Candidates(vec![candidate()]),
        AccountsReply::Availability(Availability::Unsupported),
        AccountsReply::Chosen(candidate()),
        AccountsReply::Added(account_id("cloud")),
        AccountsReply::Reauthenticated,
        AccountsReply::Grants(vec![grant()]),
        AccountsReply::Revoked,
        AccountsReply::Token(IssuedToken {
            kind: TokenKind::Bearer,
            value: SecretText::new("t"),
            expires: UnixSeconds(1),
        }),
        AccountsReply::Authenticated,
        AccountsReply::Adopted(account_id("cloud")),
        AccountsReply::Refused(Refusal::AudienceNotGranted),
        AccountsReply::Refused(Refusal::EndpointNotGranted),
    ];
    replies.iter().for_each(round_trip);
}

#[test]
fn stored_forms_keep_their_slugs() {
    let cases: Vec<(&str, String, &str)> = vec![
        ("auth kind", json(&AuthKind::OAuthPkce), r#""oauth_pkce""#),
        (
            "auth kind",
            json(&AuthKind::LoginFlowV2),
            r#""login_flow_v2""#,
        ),
        (
            "capability kind",
            json(&CapabilityKind::ImageGen),
            r#""image_gen""#,
        ),
        (
            "purpose",
            json(&SecretPurpose::OAuthRefresh),
            r#"{"kind":"oauth_refresh"}"#,
        ),
        ("delta", json(&Delta::Poll), r#""poll""#),
        (
            "locality",
            json(&Locality::OnDevice),
            r#"{"kind":"on_device"}"#,
        ),
        (
            "need",
            json(&every_need()[6]),
            r#"{"kind":"storage","v":{"access":"read_write","delta":"poll","scope":"app_folder","quota":"unreported"}}"#,
        ),
    ];
    for (name, got, want) in cases {
        assert_eq!(got, want, "{name}");
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializes")
}

#[test]
fn the_vocabulary_is_version_four() {
    assert_eq!(VocabVersion::CURRENT, VocabVersion(4));
}

#[test]
fn computer_use_slugs_are_stable() {
    let json = serde_json::to_string(&CapabilityKind::ComputerUse).expect("json");
    assert_eq!(json, "\"computer_use\"");
    assert!(CapabilityKind::ComputerUse.is_ai());
    let env = serde_json::to_string(&CuaEnv::Browser).expect("json");
    assert_eq!(env, "\"browser\"");
}

#[test]
fn the_voice_and_prompt_data_classes_have_their_slugs() {
    assert_eq!(
        serde_json::to_string(&DataClass::Voice).expect("json"),
        "\"voice\""
    );
    assert_eq!(
        serde_json::to_string(&DataClass::Prompt).expect("json"),
        "\"prompt\""
    );
}

#[test]
fn an_embedding_capability_keeps_its_prompts_with_the_model() {
    let cap = EmbedCap {
        dims: Dims(768),
        modalities: [Modality::Text].into(),
        max_input: Tokens(8_192),
        max_batch: Count(32),
        prompts: Box::new(EmbedPrompts {
            query: PrefixText("search_query: ".into()),
            document: PrefixText(String::new()),
        }),
    };
    let json = serde_json::to_string(&cap).expect("serialise");
    assert_eq!(
        json,
        r#"{"dims":768,"modalities":["text"],"max_input":8192,"max_batch":32,"prompts":{"query":"search_query: ","document":""}}"#
    );
    assert_eq!(serde_json::from_str::<EmbedCap>(&json).expect("read"), cap);
}

#[test]
fn endpoints_and_stored_documents_round_trip() {
    endpoints().iter().for_each(round_trip);
    for protocol in [
        EndpointProtocol::Imap,
        EndpointProtocol::Smtp,
        EndpointProtocol::Http,
        EndpointProtocol::Sieve,
    ] {
        round_trip(&protocol);
    }
    round_trip(&legacy_ref());
    let stored = Persisted {
        vocab: VocabVersion::CURRENT,
        accounts: vec![Account {
            id: account_id("cloud"),
            provider: ProviderId::parse("nextcloud").expect("provider"),
            label: AccountLabel("ada@example.org".into()),
            state: AccountState::Ok,
            auth: AuthKind::LoginFlowV2,
            capabilities: vec![],
            restriction: Restriction::none(),
            endpoints: endpoints(),
        }],
        grants: vec![grant()],
        toggles: vec![AccountToggle {
            account: account_id("cloud"),
            kind: CapabilityKind::Notes,
            toggle: Toggle::Off,
        }],
    };
    round_trip(&stored);
    assert_eq!(
        Persisted::from_json(&stored.to_json().expect("json")).expect("read"),
        stored
    );
}

#[test]
fn a_candidate_carries_the_endpoints_of_its_kind() {
    let json = json(&candidate());
    assert!(
        json.contains(r#""endpoints":[{"family":"imap","url":"imaps://imap.example.org","tls":"implicit","login":"ada@example.org"}"#),
        "{json}"
    );
}

fn field(kind: FieldKind, entry: Entry) -> FieldSpec {
    FieldSpec {
        kind,
        entry,
        presence: Presence::Required,
        prefill: None,
    }
}

fn review() -> Review {
    Review {
        label: AccountLabel("ada@example.org".into()),
        services: vec![
            ServiceRow {
                kind: CapabilityKind::Storage,
                state: ServiceState::Offered(Toggle::On),
                limit: Some(LimitReason::AppFolderOnly),
            },
            ServiceRow {
                kind: CapabilityKind::Notes,
                state: ServiceState::Absent(AbsentReason::NotOnServer),
                limit: None,
            },
        ],
        endpoints: endpoints(),
    }
}

#[test]
fn every_sheet_view_round_trips() {
    let nextcloud = ProviderId::parse("nextcloud").expect("provider");
    let url = EndpointUrl::parse("https://cloud.example.org/login/v2/flow/abc").expect("url");
    let views = vec![
        SheetView::Consent(ConsentAsk {
            app: app(),
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            accounts: vec![AccountChoice {
                account: account_id("cloud"),
                label: AccountLabel("Nextcloud".into()),
                provider: nextcloud.clone(),
            }],
        }),
        SheetView::Providers(vec![ProviderRow {
            id: nextcloud.clone(),
            label: "Nextcloud".into(),
            mark: "nextcloud".into(),
            kind: RowKind::Provider,
        }]),
        SheetView::SignIn(SignInView {
            provider: nextcloud.clone(),
            fields: vec![
                field(FieldKind::Address, Entry::Plain),
                field(FieldKind::Password, Entry::Secret),
            ],
            problem: Some(FieldProblem {
                field: FieldKind::Password,
                problem: ProblemKind::Refused,
            }),
        }),
        SheetView::BrowserWait {
            provider: nextcloud.clone(),
            url: WebUrl::parse("https://login.example.org/authorize?state=abc").expect("url"),
        },
        SheetView::ShowCode {
            provider: nextcloud.clone(),
            user_code: UserCode("ABCD-EFGH".into()),
            url,
        },
        SheetView::Review(ReviewView {
            provider: nextcloud.clone(),
            review: review(),
            allow: Some(app()),
        }),
        SheetView::Working(nextcloud.clone()),
        SheetView::Failed {
            provider: nextcloud,
            fault: SignInFault::NeedsClientId,
        },
        SheetView::Done,
    ];
    views.iter().for_each(round_trip);
}

#[test]
fn every_sheet_input_and_progress_round_trips() {
    let inputs = [
        SheetInput::Answer(ConsentAnswer::Deny),
        SheetInput::Pick(ProviderId::parse("nextcloud").expect("provider")),
        SheetInput::Submit(vec![
            FieldAnswer {
                kind: FieldKind::Address,
                value: FieldValue::Plain("ada@example.org".into()),
            },
            FieldAnswer {
                kind: FieldKind::Password,
                value: FieldValue::Secret(SecretText::new("pw")),
            },
        ]),
        SheetInput::Confirm(vec![ServiceChoice {
            kind: CapabilityKind::Notes,
            toggle: Toggle::Off,
        }]),
        SheetInput::Back,
        SheetInput::Retry,
        SheetInput::OpenAgain,
        SheetInput::Dismiss,
    ];
    inputs.iter().for_each(round_trip);
    assert_eq!(
        serde_json::to_string(&SheetInput::OpenAgain).expect("json"),
        r#"{"kind":"open_again"}"#
    );
    // A row from a host that predates `kind` is a provider row.
    let old: ProviderRow =
        serde_json::from_str(r#"{"id":"nextcloud","label":"Nextcloud","mark":"nextcloud"}"#)
            .expect("row");
    assert_eq!(old.kind, RowKind::Provider);
    assert_eq!(
        serde_json::to_string(&RowKind::Generic).expect("json"),
        r#""generic""#
    );
    let url = EndpointUrl::parse("https://login.example.org/device").expect("url");
    let steps = [
        Progress::Ask(vec![field(FieldKind::ApiKey, Entry::Secret)]),
        Progress::Browser(
            WebUrl::parse("https://login.example.org/authorize?client_id=a&scope=b%20c")
                .expect("url"),
        ),
        Progress::Code {
            user_code: UserCode("ABCD".into()),
            url,
        },
        Progress::Waiting,
        Progress::Review(review()),
        Progress::Done,
        Progress::Failed(SignInFault::TimedOut),
        Progress::Failed(SignInFault::StoreFailed),
    ];
    steps.iter().for_each(round_trip);
}

#[test]
fn a_typed_password_is_redacted_in_debug_and_present_on_the_wire() {
    let input = SheetInput::Submit(vec![FieldAnswer {
        kind: FieldKind::Password,
        value: FieldValue::Secret(SecretText::new("hunter2")),
    }]);
    assert!(!format!("{input:?}").contains("hunter2"));
    assert_eq!(
        json(&input),
        r#"{"kind":"submit","v":[{"kind":"password","value":{"kind":"secret","v":"hunter2"}}]}"#
    );
}

#[test]
fn new_wire_requests_keep_their_slugs() {
    assert_eq!(
        json(&AccountsRequest::OpenAuthenticated {
            grant: grant_id("g1"),
            endpoint: endpoints()[0].url.clone(),
        }),
        r#"{"kind":"open_authenticated","v":{"grant":"g1","endpoint":"imaps://imap.example.org"}}"#
    );
    assert_eq!(
        json(&AccountsRequest::OpenLinked {
            grant: grant_id("g1"),
            origin: endpoints()[0].url.clone(),
        }),
        r#"{"kind":"open_linked","v":{"grant":"g1","origin":"imaps://imap.example.org"}}"#
    );
    assert_eq!(
        json(&AccountsReply::Authenticated),
        r#"{"kind":"authenticated"}"#
    );
    assert_eq!(
        json(&AccountsReply::Refused(Refusal::EndpointNotGranted)),
        r#"{"kind":"refused","v":"endpoint_not_granted"}"#
    );
}

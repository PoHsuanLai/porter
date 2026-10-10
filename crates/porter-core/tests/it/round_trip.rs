//! Every stored or wire type of porter-core survives its serde form, and the forms that other
//! programs read keep their slugs.

use porter_core::capability::{
    Access, AgentCap, AgentProgram, AgentProtocol, Albums, Capability, CapabilityKind, CuaBatching,
    CuaCap, CuaEnv, Delta, EmbedCap, EmbedPrompts, EnvName, HashKind, IdentityCap, ImageGenCap,
    ImageMode, KeyValueCap, LabelModel, LanguageSet, LanguageTag, LibraryRead, LlmCap, LlmFeature,
    LlmWire, MailCap, MailTransport, Modality, NotesCap, NotesTransport, Offered, PhotosCap,
    PimCap, PimTransport, PrefixText, PushCap, PushChannel, QuotaReport, RerankCap, SpeechCap,
    SpeechMode, StorageCap, StorageScope, VocabVersion,
};
use porter_core::consent::{
    AccountChoice, Availability, ConsentAnswer, ConsentAsk, Decision, Grant, GrantKey, GrantScope,
    Usage, Verdict,
};
use porter_core::need::{
    AgentNeed, CuaNeed, DimsNeed, EmbedNeed, IdentityNeed, ImageGenNeed, KeyValueNeed, LlmNeed,
    MailNeed, NotesNeed, PhotosNeed, PimNeed, PushNeed, RerankNeed, SpeechNeed, StorageNeed,
};
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldProblem, FieldSpec, FieldValue, MarkColour, MarkFace,
    MarkLetter, Presence, ProblemKind, Progress, ProviderGroup, ProviderKind, ProviderRow, Review,
    ReviewView, RowKind, ServiceChoice, ServiceRow, ServiceState, SheetInput, SheetView,
    SignInFault, SignInView, UserCode,
};
use porter_core::store::{AccountToggle, Persisted};
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
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
        Capability::Agent(Box::new(claude_code())),
        Capability::Agent(Box::new(AgentCap {
            program: AgentProgram::parse("acp-agent").expect("program"),
            key_env: None,
            base_url_env: None,
            protocols: [AgentProtocol::OpenAiCompatible].into(),
        })),
    ]
}

fn claude_code() -> AgentCap {
    AgentCap {
        program: AgentProgram::parse("claude-code").expect("program"),
        key_env: Some(EnvName::parse("ANTHROPIC_API_KEY").expect("env")),
        base_url_env: Some(EnvName::parse("ANTHROPIC_BASE_URL").expect("env")),
        protocols: [AgentProtocol::AnthropicMessages].into(),
    }
}

fn every_need() -> Vec<Need> {
    let pim = PimNeed::new(Access::Read, Delta::Poll);
    vec![
        Need::Identity(IdentityNeed::new(Offered::Present, Offered::Absent)),
        Need::Mail(MailNeed::new(Access::Read, Offered::Present, Delta::Push)),
        Need::Calendar(pim.clone()),
        Need::Contacts(pim.clone()),
        Need::Tasks(pim),
        Need::Notes(NotesNeed::new(Access::ReadWrite, Delta::None)),
        Need::Storage(StorageNeed::new(
            Access::ReadWrite,
            Delta::Poll,
            StorageScope::AppFolder,
            QuotaReport::Unreported,
        )),
        Need::Photos(PhotosNeed::new(
            LibraryRead::None,
            Offered::Present,
            Albums::None,
            Offered::Absent,
            Delta::None,
        )),
        Need::Llm(LlmNeed::new([LlmFeature::Vision].into(), Tokens(32_000))),
        Need::Embeddings(EmbedNeed::new(
            DimsNeed::Exactly(Dims(768)),
            [Modality::Image].into(),
        )),
        Need::Speech(SpeechNeed::new([SpeechMode::Realtime].into())),
        Need::ImageGen(ImageGenNeed::new([ImageMode::Inpaint].into(), Px(512))),
        Need::Rerank(RerankNeed::new(Count(10))),
        Need::ComputerUse(CuaNeed::new([CuaEnv::Desktop].into())),
        Need::KeyValue(KeyValueNeed::new(Delta::Poll, Bytes(1024))),
        Need::Push(PushNeed::new()),
        Need::Agent(AgentNeed::new(
            AgentProgram::parse("claude-code").expect("program"),
            [AgentProtocol::AnthropicMessages].into(),
            Offered::Present,
        )),
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

fn restriction() -> Restriction {
    Restriction::none()
        .with_verification(Verification::Unverified {
            user_cap: Count(100),
        })
        .with_token_lifetime(TokenLifetime::SevenDays)
        .with_consent(TenantConsent::AdminRequired)
        .with_limits(vec![Limit::new(
            CapabilityKind::Photos,
            LimitReason::PickerOnly,
        )])
        .with_signed_in(UnixSeconds(1_700_000_000))
}

fn candidate() -> Candidate {
    Candidate::new(
        account_id("cloud"),
        AccountLabel("ada@example.org".into()),
        ProviderId::parse("nextcloud").expect("provider"),
        Subject::Account,
        every_capability()[6].clone(),
        restriction(),
        grant_id("g1"),
    )
    .with_endpoints(endpoints())
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
    round_trip(&Claim {
        subject: Subject::Agent(AgentProgram::parse("claude-code").expect("program")),
        offer: Offer::Present(Capability::Agent(Box::new(claude_code()))),
        provenance: Provenance::Declared,
    });
    round_trip(&Match::Short(Shortfall::Scope));
    round_trip(&Match::Short(Shortfall::Program));
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
    round_trip(&Account {
        id: account_id("claude-code"),
        provider: ProviderId::parse("claude-code").expect("provider"),
        label: AccountLabel("Claude Code".into()),
        state: AccountState::NeedsLogin,
        auth: AuthKind::AgentLogin,
        capabilities: vec![],
        restriction: Restriction::none(),
        endpoints: vec![],
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
    round_trip(&ConsentAsk::new(
        app(),
        CapabilityKind::Storage,
        DataClass::Photos,
        Usage::Interactive,
        vec![AccountChoice::new(
            account_id("cloud"),
            AccountLabel("Nextcloud".into()),
            ProviderId::parse("nextcloud").expect("provider"),
        )],
    ));
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
fn a_session_scope_round_trips_with_its_session_and_the_old_scopes_keep_their_words() {
    let session = LauncherSession::parse("sess-1").expect("session");
    let scope = GrantScope::Session(session.clone());
    round_trip(&scope);
    round_trip(&Verdict::Granted {
        grant: grant_id("g1"),
        scope: scope.clone(),
    });
    round_trip(&ConsentAnswer::Allow {
        account: account_id("cloud"),
        scope: scope.clone(),
    });
    round_trip(&Grant {
        scope: scope.clone(),
        ..grant()
    });
    assert_eq!(json(&scope), r#"{"session":"sess-1"}"#);
    assert_eq!(json(&GrantScope::Once), r#""once""#);
    assert_eq!(json(&GrantScope::Always), r#""always""#);
    for old in [r#""once""#, r#""always""#] {
        assert!(serde_json::from_str::<GrantScope>(old).is_ok(), "{old}");
    }
    // A bare `"session"` has no session to last for, and a session that is no id is refused.
    for bad in [
        r#""session""#,
        r#"{"session":"Sess 1"}"#,
        r#"{"session":""}"#,
    ] {
        assert!(serde_json::from_str::<GrantScope>(bad).is_err(), "{bad}");
    }
    // The words `Peer.Verdicts` carries beside the session.
    for (scope, word, with) in [
        (GrantScope::Once, "once", None),
        (GrantScope::Always, "always", None),
        (scope.clone(), "session", Some(&session)),
    ] {
        assert_eq!(scope.word(), word);
        assert_eq!(scope.session(), with);
        assert_eq!(GrantScope::from_words(word, with), Some(scope));
    }
    assert_eq!(GrantScope::from_words("session", None), None);
    assert_eq!(GrantScope::from_words("always", Some(&session)), None);
    assert_eq!(GrantScope::from_words("forever", None), None);
}

#[test]
fn a_consent_ask_names_its_session_only_when_it_has_one() {
    let mut ask = ConsentAsk::new(
        app(),
        CapabilityKind::Llm,
        DataClass::Prompt,
        Usage::Interactive,
        vec![AccountChoice::new(
            account_id("anthropic"),
            AccountLabel("Anthropic".into()),
            ProviderId::parse("anthropic").expect("provider"),
        )],
    );
    assert!(!json(&ask).contains("session"), "{}", json(&ask));
    // An ask an earlier build wrote has no `session` and reads as one without.
    assert_eq!(
        serde_json::from_str::<ConsentAsk>(&json(&ask)).expect("ask"),
        ask
    );
    ask.session = Some(LauncherSession::parse("sess-1").expect("session"));
    assert!(json(&ask).contains(r#""session":"sess-1""#));
    round_trip(&ask);
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
    round_trip(&Credential::Bearer(SecretText::new("api-token")));
    // The JSON a store wrote before the bearer existed still reads, and the bearer is its own
    // kind: a stored password is not read as a token.
    for old in [
        r#"{"kind":"password","v":"pw"}"#,
        r#"{"kind":"api_key","v":"k"}"#,
        r#"{"kind":"key_pair","v":{"access_key":"A","secret":"s"}}"#,
    ] {
        serde_json::from_str::<Credential>(old).expect(old);
    }
    assert_eq!(
        serde_json::to_string(&Credential::Bearer(SecretText::new("t"))).expect("json"),
        r#"{"kind":"bearer","v":"t"}"#
    );
    assert_eq!(
        serde_json::from_str::<Credential>(r#"{"kind":"password","v":"pw"}"#).expect("old"),
        Credential::Password(SecretText::new("pw"))
    );
    round_trip(&IssuedToken::new(
        TokenKind::Xoauth2,
        SecretText::new("t"),
        UnixSeconds(9),
    ));
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
    ];
    requests.iter().for_each(round_trip);
    let replies = vec![
        AccountsReply::Candidates(vec![candidate()]),
        AccountsReply::Availability(Availability::Unsupported),
        AccountsReply::Chosen(candidate()),
        AccountsReply::Added(account_id("cloud")),
        AccountsReply::AlreadyAdded(account_id("cloud")),
        AccountsReply::Reauthenticated,
        AccountsReply::Grants(vec![grant()]),
        AccountsReply::Revoked,
        AccountsReply::Token(IssuedToken::new(
            TokenKind::Bearer,
            SecretText::new("t"),
            UnixSeconds(1),
        )),
        AccountsReply::Authenticated,
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
fn the_vocabulary_is_version_twelve() {
    assert_eq!(VocabVersion::CURRENT, VocabVersion(12));
}

#[test]
fn a_provider_row_names_its_group_only_when_it_has_one_and_an_old_row_reads_as_none() {
    let mut row = ProviderRow {
        id: ProviderId::parse("openai").expect("provider"),
        label: "OpenAI".into(),
        mark: "openai".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: None,
    };
    assert!(!json(&row).contains("group"), "{}", json(&row));
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    // What the build before `group` wrote.
    let old =
        r#"{"id":"openai","label":"OpenAI","mark":"openai","kind":"provider","auth":"service"}"#;
    assert_eq!(
        serde_json::from_str::<ProviderRow>(old).expect("old").group,
        None
    );
    for (group, slug) in [
        (ProviderGroup::Internet, "internet"),
        (ProviderGroup::Intelligence, "intelligence"),
        (ProviderGroup::Agent, "agent"),
    ] {
        row.group = Some(group);
        assert!(
            json(&row).contains(&format!(r#""group":"{slug}""#)),
            "{}",
            json(&row)
        );
        assert_eq!(
            serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
            row
        );
    }
    let bad = r#"{"id":"openai","label":"OpenAI","mark":"openai","group":"cloud"}"#;
    assert!(serde_json::from_str::<ProviderRow>(bad).is_err());
}

#[test]
fn a_provider_row_names_its_face_only_when_it_has_one_and_an_old_row_reads_as_none() {
    let mut row = ProviderRow {
        id: ProviderId::parse("openai").expect("provider"),
        label: "OpenAI".into(),
        mark: "openai".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: None,
    };
    assert!(!json(&row).contains("mark_face"), "{}", json(&row));
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    let old =
        r#"{"id":"openai","label":"OpenAI","mark":"openai","kind":"provider","auth":"service"}"#;
    assert_eq!(serde_json::from_str::<ProviderRow>(old).expect("old"), row);
    row.mark_face = Some(MarkFace {
        letter: MarkLetter::parse("O").expect("letter"),
        colour: MarkColour::parse("#10a37f").expect("colour"),
    });
    assert!(
        json(&row).contains(r##""mark_face":{"letter":"O","colour":"#10A37F"}"##),
        "{}",
        json(&row)
    );
    assert_eq!(
        serde_json::from_str::<ProviderRow>(&json(&row)).expect("row"),
        row
    );
    let bad = r##"{"id":"openai","label":"OpenAI","mark":"openai","mark_face":{"letter":"OPE","colour":"#10A37F"}}"##;
    assert!(serde_json::from_str::<ProviderRow>(bad).is_err());
}

#[test]
fn an_app_label_is_named_only_when_accountd_has_one_and_an_earlier_ask_reads_without_it() {
    let mut ask = ConsentAsk::new(
        app(),
        CapabilityKind::Llm,
        DataClass::Prompt,
        Usage::Interactive,
        vec![AccountChoice::new(
            account_id("anthropic"),
            AccountLabel("Anthropic".into()),
            ProviderId::parse("anthropic").expect("provider"),
        )],
    );
    assert!(!json(&ask).contains("app_label"), "{}", json(&ask));
    assert_eq!(
        serde_json::from_str::<ConsentAsk>(&json(&ask)).expect("ask"),
        ask
    );
    ask.app_label = Some(AppLabel("Claude Code".into()));
    assert!(
        json(&ask).contains(r#""app_label":"Claude Code""#),
        "{}",
        json(&ask)
    );
    assert_eq!(
        serde_json::from_str::<ConsentAsk>(&json(&ask)).expect("ask"),
        ask
    );
}

#[test]
fn a_sign_in_fault_keeps_the_slug_of_an_account_already_there() {
    assert_eq!(json(&SignInFault::AlreadyAdded), r#""already_added""#);
}

#[test]
fn the_tailscale_values_keep_their_slugs() {
    // Vocabulary 12: the sign-in a program on this computer holds, and the three faults a
    // sheet words for it.
    assert_eq!(json(&AuthKind::OwnProgram), r#""own_program""#);
    assert_eq!(json(&SignInFault::NotRunning), r#""not_running""#);
    assert_eq!(json(&SignInFault::SignedOut), r#""signed_out""#);
    assert_eq!(json(&SignInFault::NotAllowed), r#""not_allowed""#);
    assert_eq!(
        serde_json::from_str::<AuthKind>(r#""own_program""#).expect("kind"),
        AuthKind::OwnProgram
    );
}

#[test]
fn agent_values_keep_their_slugs_and_hold_no_secret() {
    let cases = [
        ("auth kind", json(&AuthKind::AgentLogin), r#""agent_login""#),
        ("state", json(&AccountState::NeedsLogin), r#""needs_login""#),
        ("agent state", json(&AgentState::Ready), r#""ready""#),
        (
            "agent state",
            json(&AgentState::NeedsLogin),
            r#""needs_login""#,
        ),
        ("kind", json(&CapabilityKind::Agent), r#""agent""#),
        ("family", json(&Family::AcpAgent), r#""acp_agent""#),
        (
            "protocol",
            json(&AgentProtocol::OpenAiCompatible),
            r#""openai_compatible""#,
        ),
        (
            "capability",
            json(&Capability::Agent(Box::new(claude_code()))),
            r#"{"kind":"agent","v":{"program":"claude-code","key_env":"ANTHROPIC_API_KEY","base_url_env":"ANTHROPIC_BASE_URL","protocols":["anthropic_messages"]}}"#,
        ),
        (
            "subject",
            json(&Subject::Agent(
                AgentProgram::parse("codex").expect("program"),
            )),
            r#"{"kind":"agent","v":"codex"}"#,
        ),
    ];
    for (name, got, want) in cases {
        assert_eq!(got, want, "{name}");
    }
    assert!(!CapabilityKind::Agent.is_ai());
    for bad in ["", "Claude", "1x", "a b", "claude_code", &"a".repeat(49)] {
        assert!(AgentProgram::parse(bad).is_err(), "{bad:?}");
    }
    for bad in ["", "anthropic_api_key", "1KEY", "A-B", "A B"] {
        assert!(EnvName::parse(bad).is_err(), "{bad:?}");
    }
    assert!(serde_json::from_str::<AgentProgram>("\"Not Ok\"").is_err());
    assert!(AccountState::NeedsLogin.needs_person() && AccountState::NeedsReauth.needs_person());
    assert!(!AccountState::Ok.needs_person() && !AccountState::Offline.needs_person());
    assert_eq!(AgentState::Ready.account_state(), AccountState::Ok);
    assert_eq!(
        AgentState::NeedsLogin.account_state(),
        AccountState::NeedsLogin
    );
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
fn the_tasks_data_class_has_its_slug_and_old_slugs_still_read() {
    assert_eq!(
        serde_json::to_string(&DataClass::Tasks).expect("json"),
        "\"tasks\""
    );
    for (slug, class) in [
        ("calendar", DataClass::Calendar),
        ("contacts", DataClass::Contacts),
        ("tasks", DataClass::Tasks),
        ("notes", DataClass::Notes),
    ] {
        let read: DataClass = serde_json::from_str(&format!("\"{slug}\"")).expect("slug");
        assert_eq!(read, class);
    }
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
    let row = Some(ProviderRow {
        id: nextcloud.clone(),
        label: "Nextcloud".into(),
        mark: "nextcloud".into(),
        kind: RowKind::Provider,
        auth: ProviderKind::Service,
        mark_face: None,
        group: Some(ProviderGroup::Internet),
    });
    let views = vec![
        SheetView::Consent(
            ConsentAsk::new(
                app(),
                CapabilityKind::Storage,
                DataClass::Photos,
                Usage::Interactive,
                vec![AccountChoice::new(
                    account_id("cloud"),
                    AccountLabel("Nextcloud".into()),
                    nextcloud.clone(),
                )],
            )
            .with_session(LauncherSession::parse("sess-1").expect("session"))
            .with_app_label(AppLabel("Photos".into())),
        ),
        SheetView::Providers(vec![ProviderRow {
            id: nextcloud.clone(),
            label: "Nextcloud".into(),
            mark: "nextcloud".into(),
            kind: RowKind::Provider,
            auth: ProviderKind::AgentLogin,
            mark_face: Some(MarkFace {
                letter: MarkLetter::parse("Cx").expect("letter"),
                colour: MarkColour::parse("#10a37f").expect("colour"),
            }),
            group: Some(ProviderGroup::Agent),
        }]),
        SheetView::SignIn(SignInView {
            provider: nextcloud.clone(),
            row: row.clone(),
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
            row: row.clone(),
            url: WebUrl::parse("https://login.example.org/authorize?state=abc").expect("url"),
        },
        SheetView::ShowCode {
            provider: nextcloud.clone(),
            row: row.clone(),
            user_code: UserCode("ABCD-EFGH".into()),
            url,
        },
        SheetView::Review(ReviewView {
            provider: nextcloud.clone(),
            row: row.clone(),
            review: review(),
            allow: Some(app()),
            allow_label: Some(AppLabel("Photos".into())),
        }),
        SheetView::Working {
            provider: nextcloud.clone(),
            row: row.clone(),
        },
        SheetView::Working {
            provider: nextcloud.clone(),
            row: None,
        },
        SheetView::Failed {
            provider: nextcloud,
            row,
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

#[test]
fn a_provider_row_and_views_written_before_the_kind_and_name_still_read() {
    let old_row = r#"{"id":"claude-code","label":"Claude Code","mark":"claude","kind":"provider"}"#;
    let row: ProviderRow = serde_json::from_str(old_row).expect("old row");
    assert_eq!(row.auth, ProviderKind::Service);
    let old_view = r#"{"kind":"browser_wait","v":{"provider":"nextcloud","url":"https://login.example.org/a"}}"#;
    match serde_json::from_str::<SheetView>(old_view).expect("old view") {
        SheetView::BrowserWait { row, .. } => assert_eq!(row, None),
        other => panic!("{other:?}"),
    }
    let agent = r#"{"id":"claude-code","label":"Claude Code","mark":"c","auth":"agent_login"}"#;
    let row: ProviderRow = serde_json::from_str(agent).expect("row");
    assert_eq!(
        (row.auth, row.label.as_str()),
        (ProviderKind::AgentLogin, "Claude Code")
    );
}

#[test]
fn every_auth_kind_has_one_way_to_sign_in_again_and_its_slug_is_its_serde_form() {
    let table = [
        (AuthKind::OAuthPkce, SignInWay::Browser),
        (AuthKind::OAuthMintsKey, SignInWay::Browser),
        (AuthKind::OAuthPlan, SignInWay::Browser),
        (AuthKind::LoginFlowV2, SignInWay::Browser),
        (AuthKind::Password, SignInWay::Password),
        (AuthKind::AppPassword, SignInWay::Password),
        (AuthKind::LocalBridge, SignInWay::Password),
        (AuthKind::ApiKey, SignInWay::Key),
        (AuthKind::KeyPair, SignInWay::Key),
        (AuthKind::AgentLogin, SignInWay::Agent),
        (AuthKind::CloudIdentity, SignInWay::Outside),
        (AuthKind::OwnProgram, SignInWay::Outside),
        (AuthKind::None, SignInWay::Nothing),
        (AuthKind::LocalRuntime, SignInWay::Nothing),
    ];
    for (kind, way) in table {
        assert_eq!(kind.sign_in_way(), way, "{kind:?}");
        assert_eq!(json(&way), format!("\"{}\"", way.slug()), "{way:?}");
    }
}

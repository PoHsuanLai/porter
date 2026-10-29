//! A candidate survives its D-Bus shape, and the shape is what the introspection says: a path, a
//! label and a vardict.

use porter_capability::{
    Access, AgentCap, AgentProgram, AgentProtocol, Capability, CapabilityKind, Delta, HashKind,
    LlmCap, LlmFeature, LlmWire, ModelId, StorageCap, StorageScope, Subject,
};
use porter_core::{AccountId, GrantId};
use porter_core::{
    Candidate, EndpointUrl, Family, Limit, LimitReason, LoginName, Restriction, ServiceEndpoint,
    TenantConsent, Tls, TokenLifetime, Verification,
};
use porter_dbus::accounts::{account_path, candidate_from_dbus, candidate_to_dbus};
use porter_ids::{Count, Tokens};
use zbus::zvariant::OwnedObjectPath;

fn candidates() -> Vec<Candidate> {
    let storage = Capability::Storage(StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: true,
        scope: StorageScope::AppFolder,
        hashes: HashKind::QuickXor,
        ranges: true,
        chunked_upload: true,
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
    let (back, _): (porter_dbus::accounts::CandidateArg, usize) =
        bytes.deserialize().expect("decodes");
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
        let (back, _): (porter_dbus::accounts::CandidateArg, usize) =
            bytes.deserialize().expect("decodes");
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

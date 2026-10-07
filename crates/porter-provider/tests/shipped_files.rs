//! Every provider file the repo ships parses, round-trips through TOML, and lays over the
//! others by id.

use porter_provider::{ProviderSet, ProviderSpec, parse_provider};
use std::path::PathBuf;

fn shipped() -> Vec<(PathBuf, ProviderSpec)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("providers directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("readable");
            let spec = parse_provider(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (path, spec)
        })
        .collect()
}

#[test]
fn every_shipped_provider_file_parses_and_is_named_by_its_id() {
    let files = shipped();
    assert_eq!(files.len(), 23);
    for (path, spec) in files {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        assert_eq!(spec.id.as_str(), stem, "{}", path.display());
    }
}

#[test]
fn a_provider_spec_round_trips_through_toml() {
    for (path, spec) in shipped() {
        let text = toml::to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            parse_provider(&text).as_ref(),
            Ok(&spec),
            "{}",
            path.display()
        );
    }
}

#[test]
fn a_user_file_replaces_the_system_file_of_the_same_id() {
    let specs: Vec<ProviderSpec> = shipped().into_iter().map(|(_, spec)| spec).collect();
    let mut mine = specs[0].clone();
    mine.label = "My own".into();
    let set = ProviderSet::layered(specs.clone(), vec![mine.clone()]);
    assert_eq!(set.specs().len(), specs.len());
    assert_eq!(set.get(&mine.id).map(|s| s.label.as_str()), Some("My own"));
}

#[test]
fn the_microsoft_file_declares_mail_over_imap_and_the_rest_over_graph() {
    use porter_core::Family;
    use porter_core::capability::CapabilityKind as K;
    let spec = shipped()
        .into_iter()
        .map(|(_, spec)| spec)
        .find(|spec| spec.id.as_str() == "microsoft")
        .expect("microsoft.toml ships");
    let rows: Vec<(K, Family)> = spec
        .capabilities
        .iter()
        .map(|row| (row.capability.kind(), row.family))
        .collect();
    assert_eq!(
        rows,
        [
            (K::Mail, Family::Imap),
            (K::Calendar, Family::Graph),
            (K::Contacts, Family::Graph),
            (K::Tasks, Family::Graph),
            (K::Notes, Family::Graph),
            (K::Storage, Family::Graph),
        ]
    );
    assert_eq!(spec.auth.issuer, Some(porter_provider::Issuer::Microsoft));
    assert!(
        spec.matching
            .domains
            .iter()
            .any(|d| d.as_str() == "outlook.com")
    );
}

fn shipped_spec(id: &str) -> ProviderSpec {
    shipped()
        .into_iter()
        .map(|(_, spec)| spec)
        .find(|spec| spec.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} ships"))
}

#[test]
fn the_generic_files_declare_what_the_generic_family_finds() {
    use porter_core::{AuthKind, Family};
    use porter_provider::Discovery;
    let families = |spec: &ProviderSpec| -> Vec<Family> {
        spec.capabilities.iter().map(|row| row.family).collect()
    };
    let imap = shipped_spec("generic-imap");
    assert_eq!(imap.auth.kind, AuthKind::Password);
    assert_eq!(imap.discovery, Discovery::Autoconfig);
    assert_eq!(families(&imap), [Family::Imap]);
    let dav = shipped_spec("generic-dav");
    assert_eq!(dav.auth.kind, AuthKind::Password);
    assert_eq!(dav.discovery, Discovery::WellKnown);
    assert_eq!(families(&dav), [Family::CalDav, Family::CardDav]);
    // They are the fallback: no address is theirs by name, and a file that names domains
    // (Fastmail, iCloud) claims it first.
    for spec in [imap, dav] {
        assert!(spec.matching.domains.is_empty(), "{}", spec.id);
        assert!(spec.matching.mx_suffixes.is_empty(), "{}", spec.id);
        assert!(
            spec.capabilities.iter().all(|row| row.endpoint.is_none()),
            "{}",
            spec.id
        );
    }
}

#[test]
fn the_nextcloud_file_names_the_login_flow_and_no_server() {
    use porter_core::AuthKind;
    use porter_provider::Discovery;
    let nextcloud = shipped_spec("nextcloud");
    assert_eq!(nextcloud.auth.kind, AuthKind::LoginFlowV2);
    assert_eq!(nextcloud.discovery, Discovery::NextcloudOcs);
    // A Nextcloud is wherever the person says: no row has an endpoint, so the sign-in asks.
    assert!(
        nextcloud
            .capabilities
            .iter()
            .all(|row| row.endpoint.is_none())
    );
}

#[test]
fn the_ai_company_files_declare_a_cloud_llm_row_behind_a_pasted_key() {
    use porter_core::capability::CapabilityKind as K;
    use porter_core::{AuthKind, Family, Locality};
    use porter_provider::Discovery;
    for (id, label, family, endpoint) in [
        (
            "anthropic",
            "Anthropic (Claude)",
            Family::Messages,
            "https://api.anthropic.com/v1",
        ),
        (
            "google-ai",
            "Google (Gemini)",
            Family::GenerateContent,
            "https://generativelanguage.googleapis.com/v1beta",
        ),
        (
            "moonshot",
            "Moonshot (Kimi)",
            Family::ChatCompletions,
            "https://api.moonshot.ai/v1",
        ),
        (
            "openrouter",
            "OpenRouter",
            Family::ChatCompletions,
            "https://openrouter.ai/api/v1",
        ),
        (
            "openai",
            "OpenAI",
            Family::ChatCompletions,
            "https://api.openai.com/v1",
        ),
    ] {
        let spec = shipped_spec(id);
        assert_eq!(spec.label, label, "{id}");
        assert_eq!(
            (spec.auth.kind, spec.auth.issuer),
            (AuthKind::ApiKey, None),
            "{id}"
        );
        assert_eq!(spec.discovery, Discovery::ModelList, "{id}");
        // Keys are not found from an email address.
        assert!(spec.matching.domains.is_empty() && spec.matching.mx_suffixes.is_empty());
        assert_eq!(
            spec.ai.as_ref().map(|ai| ai.locality.clone()),
            Some(Locality::Cloud { region: None }),
            "{id}"
        );
        // The model row first, then the agent programs allowed to use the key.
        assert!(!spec.capabilities.is_empty(), "{id}");
        let row = &spec.capabilities[0];
        assert_eq!(
            (row.capability.kind(), row.family),
            (K::Llm, family),
            "{id}"
        );
        assert_eq!(
            row.endpoint.as_ref().map(|e| e.0.as_str()),
            Some(endpoint),
            "{id}"
        );
    }
}

#[test]
fn the_brand_files_declare_their_servers_and_how_their_accounts_sign_in() {
    use porter_core::{AuthKind, Family};
    use porter_provider::Discovery;
    // (id, auth, imap endpoint, smtp endpoint): mailo's presets table became these files.
    const CASES: &[(&str, AuthKind, &str, &str)] = &[
        (
            "fastmail",
            AuthKind::AppPassword,
            "imaps://imap.fastmail.com:993",
            "smtps://smtp.fastmail.com:465",
        ),
        (
            "icloud",
            AuthKind::AppPassword,
            "imaps://imap.mail.me.com:993",
            "smtp://smtp.mail.me.com:587",
        ),
        (
            "yahoo",
            AuthKind::AppPassword,
            "imaps://imap.mail.yahoo.com:993",
            "smtps://smtp.mail.yahoo.com:465",
        ),
        (
            "gmx",
            AuthKind::Password,
            "imaps://imap.gmx.com:993",
            "smtps://mail.gmx.com:465",
        ),
    ];
    for (id, auth, imap, smtp) in CASES {
        let spec = shipped_spec(id);
        assert_eq!(spec.auth.kind, *auth, "{id}");
        assert_eq!(spec.discovery, Discovery::Fixed, "{id}");
        assert!(!spec.matching.domains.is_empty(), "{id} claims its domains");
        let endpoint = |family: Family| {
            spec.capabilities
                .iter()
                .find(|row| row.family == family)
                .and_then(|row| row.endpoint.as_ref())
                .map(|e| e.0.as_str())
        };
        assert_eq!(endpoint(Family::Imap), Some(*imap), "{id}");
        assert_eq!(endpoint(Family::Smtp), Some(*smtp), "{id}");
    }
    let jmap = shipped_spec("generic-jmap");
    assert_eq!(jmap.auth.kind, AuthKind::Password);
    assert_eq!(jmap.discovery, Discovery::JmapSession);
    assert_eq!(
        jmap.capabilities
            .iter()
            .map(|row| row.family)
            .collect::<Vec<_>>(),
        [Family::Jmap]
    );
    assert!(jmap.capabilities.iter().all(|row| row.endpoint.is_none()));
}

#[test]
fn the_compiled_in_files_are_the_whole_providers_directory_byte_for_byte() {
    let on_disk = shipped();
    assert_eq!(porter_provider::SHIPPED_FILES.len(), on_disk.len());
    // By id, not by position: `google-ai.toml` sorts before `google.toml` as a path.
    for (path, _) in &on_disk {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        let (_, text) = porter_provider::SHIPPED_FILES
            .iter()
            .find(|(id, _)| *id == stem)
            .unwrap_or_else(|| panic!("{stem} is not compiled in"));
        assert_eq!(
            *text,
            std::fs::read_to_string(path).expect("readable"),
            "{stem}"
        );
    }
    let set = porter_provider::shipped();
    assert_eq!(set.specs().len(), on_disk.len());
    for (id, _) in porter_provider::SHIPPED_FILES {
        let id = porter_core::ProviderId::parse(id).expect("id");
        assert!(set.get(&id).is_some(), "{id}");
    }
}

#[test]
fn only_microsofts_storage_row_declares_linked_origins_and_asks_the_app_folder_only() {
    use porter_core::CapabilityKind;
    let rows: Vec<(String, CapabilityKind, Vec<String>)> = shipped()
        .into_iter()
        .flat_map(|(_, spec)| {
            let id = spec.id.as_str().to_owned();
            spec.capabilities.into_iter().map(move |row| {
                let origins = row.linked_origins.iter().map(ToString::to_string).collect();
                (id.clone(), row.capability.kind(), origins)
            })
        })
        .filter(|(_, _, origins): &(String, CapabilityKind, Vec<String>)| !origins.is_empty())
        .collect();
    assert_eq!(
        rows,
        [(
            "microsoft".to_owned(),
            CapabilityKind::Storage,
            vec![
                "*.up.1drv.com".to_owned(),
                "*.files.1drv.com".to_owned(),
                "*.sharepoint.com".to_owned()
            ]
        )]
    );
}

#[test]
fn the_generic_files_make_generic_rows_and_every_other_file_a_provider_row() {
    use porter_core::sheet::RowKind;
    let rows: Vec<_> = shipped().iter().map(|(_, spec)| spec.sheet_row()).collect();
    let generic: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == RowKind::Generic)
        .map(|r| r.id.as_str().to_owned())
        .collect();
    assert!(!generic.is_empty());
    assert!(generic.iter().all(|id| id.starts_with("generic-")));
    assert!(
        rows.iter()
            .filter(|r| r.id.as_str().starts_with("generic-"))
            .all(|r| r.kind == RowKind::Generic)
    );
    assert!(
        rows.iter()
            .filter(|r| !r.id.as_str().starts_with("generic-"))
            .all(|r| r.kind == RowKind::Provider)
    );
}

#[test]
fn the_agent_files_hold_no_secret_and_name_their_program_and_variables() {
    use porter_core::capability::{AgentProtocol as P, CapabilityKind as K};
    use porter_core::{AuthKind, Capability, Family};
    use porter_provider::Discovery;
    // (id, program, key variable, base-url variable, protocols)
    const CASES: &[(&str, &str, Option<&str>, Option<&str>, &[P])] = &[
        (
            "claude-code",
            "claude-code",
            Some("ANTHROPIC_API_KEY"),
            Some("ANTHROPIC_BASE_URL"),
            &[P::AnthropicMessages],
        ),
        (
            "gemini-cli",
            "gemini-cli",
            Some("GEMINI_API_KEY"),
            None,
            &[P::GenerateContent],
        ),
        (
            "codex",
            "codex",
            Some("OPENAI_API_KEY"),
            Some("OPENAI_BASE_URL"),
            &[P::OpenAiCompatible],
        ),
        ("acp-agent", "acp-agent", None, None, &[P::OpenAiCompatible]),
    ];
    for (id, program, key, base_url, protocols) in CASES {
        let spec = shipped_spec(id);
        assert_eq!(
            (spec.auth.kind, spec.auth.issuer),
            (AuthKind::AgentLogin, None),
            "{id}"
        );
        assert_eq!(spec.discovery, Discovery::Fixed, "{id}");
        assert!(spec.ai.is_none(), "{id}");
        assert_eq!(spec.capabilities.len(), 1, "{id}");
        let row = &spec.capabilities[0];
        assert_eq!(
            (row.capability.kind(), row.family),
            (K::Agent, Family::AcpAgent)
        );
        assert!(
            row.endpoint.is_none() && row.linked_origins.is_empty(),
            "{id}"
        );
        let Capability::Agent(agent) = &row.capability else {
            panic!("{id}: not an agent row");
        };
        assert_eq!(agent.program.as_str(), *program, "{id}");
        assert_eq!(agent.key_env.as_ref().map(|n| n.as_str()), *key, "{id}");
        assert_eq!(
            agent.base_url_env.as_ref().map(|n| n.as_str()),
            *base_url,
            "{id}"
        );
        assert_eq!(agent.protocols, protocols.iter().copied().collect(), "{id}");
    }
}

#[test]
fn a_key_provider_names_the_agent_programs_that_may_use_its_key() {
    use porter_core::Capability;
    use porter_core::capability::CapabilityKind as K;
    // Every key provider's named programs, by file id; a key file that names none is listed empty.
    let named = |id: &str| -> Vec<String> {
        shipped_spec(id)
            .capabilities
            .iter()
            .filter_map(|row| match &row.capability {
                Capability::Agent(agent) => Some(agent.program.to_string()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(named("anthropic"), ["claude-code"]);
    assert_eq!(named("openai"), ["codex"]);
    assert_eq!(named("google-ai"), ["gemini-cli"]);
    assert!(named("openrouter").is_empty());
    assert!(named("moonshot").is_empty());
    // The model row is still the first row of a key provider.
    for id in ["anthropic", "openai", "google-ai"] {
        assert_eq!(shipped_spec(id).capabilities[0].capability.kind(), K::Llm);
    }
    // The program each key file names is the program of the agent file that shares its vendor,
    // with the same variables: one fact, written twice, must agree.
    for (key_file, agent_file) in [
        ("anthropic", "claude-code"),
        ("openai", "codex"),
        ("google-ai", "gemini-cli"),
    ] {
        let agent_cap = |id: &str| {
            shipped_spec(id)
                .capabilities
                .iter()
                .find_map(|row| match &row.capability {
                    Capability::Agent(agent) => Some(agent.clone()),
                    _ => None,
                })
                .expect("an agent row")
        };
        assert_eq!(agent_cap(key_file), agent_cap(agent_file), "{key_file}");
    }
}

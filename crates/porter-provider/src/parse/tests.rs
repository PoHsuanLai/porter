use super::*;
use crate::spec::{Discovery, Port};
use porter_core::capability::{Access, Delta, StorageScope};
use porter_core::{AuthKind, Capability, Family};

const HEAD: &str = r#"
id = "example"
label = "Example"
mark = "example"
"#;

const STORAGE_ROW: &str = r#"
[[capability]]
family = "webdav"
kind = "storage"
v = { access = "read_write", delta = "poll", quota = "unreported", scope = "full", hashes = "none", ranges = "absent", chunked_upload = "absent" }
"#;

const LLM_ROW: &str = r#"
[[capability]]
family = "chat_completions"
kind = "llm"
v = { features = ["chat", "tools"], context = 8192, max_output = 1024, wire = "chat_completions" }
"#;

const AGENT_ROW: &str = r#"
[[capability]]
family = "acp_agent"
kind = "agent"
v = { program = "claude-code", key_env = "ANTHROPIC_API_KEY", base_url_env = "ANTHROPIC_BASE_URL", protocols = ["anthropic_messages"] }
"#;

const AI: &str = r#"
[ai]
locality = { kind = "cloud", v = { region = "eu-west-1" } }
billing = { kind = "metered", v = { input_per_mtok = 3000000, output_per_mtok = 15000000 } }
"#;

fn file(auth: &str, discovery: &str, rest: &[&str]) -> String {
    format!(
        "{HEAD}\n[auth]\n{auth}\n\n[discovery]\n{discovery}\n{}",
        rest.concat()
    )
}

#[test]
fn a_storage_provider_parses_to_its_typed_rows() {
    let text = file(
        r#"kind = "app_password""#,
        r#"kind = "well_known""#,
        &[STORAGE_ROW],
    );
    let spec = parse_provider(&text).expect("parses");
    assert_eq!(spec.auth.kind, AuthKind::AppPassword);
    assert_eq!(spec.discovery, Discovery::WellKnown);
    assert_eq!(spec.capabilities.len(), 1);
    assert_eq!(spec.capabilities[0].family, Family::WebDav);
    let Capability::Storage(storage) = &spec.capabilities[0].capability else {
        panic!("not storage: {:?}", spec.capabilities[0].capability);
    };
    assert_eq!(
        (storage.access, storage.delta, storage.scope),
        (Access::ReadWrite, Delta::Poll, StorageScope::Full)
    );
}

#[test]
fn probe_ports_carry_their_ports() {
    let text = file(
        r#"kind = "local_runtime""#,
        "kind = \"probe_ports\"\nv = { ports = [11434, 8080] }",
        &[AI, LLM_ROW],
    );
    let spec = parse_provider(&text).expect("parses");
    assert_eq!(
        spec.discovery,
        Discovery::ProbePorts {
            ports: vec![Port(11434), Port(8080)]
        }
    );
}

#[test]
fn supervised_discovery_is_a_plain_kind() {
    let text = file(
        r#"kind = "local_runtime""#,
        r#"kind = "supervised""#,
        &[AI, LLM_ROW],
    );
    let spec = parse_provider(&text).expect("parses");
    assert_eq!(spec.discovery, Discovery::Supervised);
}

#[test]
fn provider_files_are_checked() {
    let oauth_with_issuer = r#"kind = "oauth_pkce"
issuer = "microsoft""#;
    let cases: Vec<(&str, String, Result<(), ProviderFileError>)> = vec![
        (
            "a storage provider",
            file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW]),
            Ok(()),
        ),
        (
            "oauth with its issuer",
            file(oauth_with_issuer, r#"kind = "fixed""#, &[STORAGE_ROW]),
            Ok(()),
        ),
        (
            "an ai provider with its [ai] table",
            file(
                r#"kind = "api_key""#,
                r#"kind = "model_list""#,
                &[AI, LLM_ROW],
            ),
            Ok(()),
        ),
        (
            "oauth without an issuer",
            file(
                r#"kind = "oauth_pkce""#,
                r#"kind = "fixed""#,
                &[STORAGE_ROW],
            ),
            Err(ProviderFileError::IssuerMismatch(id())),
        ),
        (
            "an issuer on a password provider",
            file(
                "kind = \"password\"\nissuer = \"google\"",
                r#"kind = "fixed""#,
                &[STORAGE_ROW],
            ),
            Err(ProviderFileError::IssuerMismatch(id())),
        ),
        (
            "no capability rows",
            file(r#"kind = "password""#, r#"kind = "fixed""#, &[])
                .replace("mark = \"example\"", "mark = \"example\"\ncapability = []"),
            Err(ProviderFileError::NoCapabilities(id())),
        ),
        (
            "a program that holds its own sign-in may list no capability",
            file(r#"kind = "own_program""#, r#"kind = "fixed""#, &[])
                .replace("mark = \"example\"", "mark = \"example\"\ncapability = []"),
            Ok(()),
        ),
        (
            "an llm row without [ai]",
            file(r#"kind = "api_key""#, r#"kind = "model_list""#, &[LLM_ROW]),
            Err(ProviderFileError::AiSpecMismatch(id())),
        ),
        (
            "an agent that signs itself in",
            file(r#"kind = "agent_login""#, r#"kind = "fixed""#, &[AGENT_ROW]),
            Ok(()),
        ),
        (
            "a key provider that names an agent beside its model row",
            file(
                r#"kind = "api_key""#,
                r#"kind = "model_list""#,
                &[AI, LLM_ROW, AGENT_ROW],
            ),
            Ok(()),
        ),
        (
            "an agent login provider with a storage row too",
            file(
                r#"kind = "agent_login""#,
                r#"kind = "fixed""#,
                &[AGENT_ROW, STORAGE_ROW],
            ),
            Err(ProviderFileError::AgentRows(id())),
        ),
        (
            "an agent row served by another family",
            file(
                r#"kind = "agent_login""#,
                r#"kind = "fixed""#,
                &[&AGENT_ROW.replace("acp_agent", "messages")],
            ),
            Err(ProviderFileError::AgentRows(id())),
        ),
        (
            "a family acp_agent row that is not an agent",
            file(
                r#"kind = "password""#,
                r#"kind = "fixed""#,
                &[&STORAGE_ROW.replace("webdav", "acp_agent")],
            ),
            Err(ProviderFileError::AgentRows(id())),
        ),
        (
            "the same program named twice",
            file(
                r#"kind = "api_key""#,
                r#"kind = "model_list""#,
                &[AI, LLM_ROW, AGENT_ROW, AGENT_ROW],
            ),
            Err(ProviderFileError::AgentRows(id())),
        ),
        (
            "[ai] on a storage provider",
            file(
                r#"kind = "password""#,
                r#"kind = "fixed""#,
                &[AI, STORAGE_ROW],
            ),
            Err(ProviderFileError::AiSpecMismatch(id())),
        ),
    ];
    for (name, text, expected) in cases {
        assert_eq!(parse_provider(&text).map(|_| ()), expected, "{name}");
    }
}

#[test]
fn an_agent_row_parses_to_its_typed_fields() {
    use porter_core::capability::{AgentProgram, AgentProtocol};
    let text = file(r#"kind = "agent_login""#, r#"kind = "fixed""#, &[AGENT_ROW]);
    let spec = parse_provider(&text).expect("parses");
    assert_eq!(spec.auth.kind, AuthKind::AgentLogin);
    assert!(spec.ai.is_none());
    let Capability::Agent(agent) = &spec.capabilities[0].capability else {
        panic!("not an agent: {:?}", spec.capabilities[0].capability);
    };
    assert_eq!(spec.capabilities[0].family, Family::AcpAgent);
    assert_eq!(
        agent.program,
        AgentProgram::parse("claude-code").expect("program")
    );
    assert_eq!(
        agent.key_env.as_ref().map(|n| n.as_str()),
        Some("ANTHROPIC_API_KEY")
    );
    assert_eq!(
        agent.base_url_env.as_ref().map(|n| n.as_str()),
        Some("ANTHROPIC_BASE_URL")
    );
    assert_eq!(agent.protocols, [AgentProtocol::AnthropicMessages].into());
}

#[test]
fn unknown_words_are_syntax_errors() {
    let cases = [
        (
            "unknown auth kind",
            file(r#"kind = "magic""#, r#"kind = "fixed""#, &[STORAGE_ROW]),
        ),
        (
            "unknown family",
            file(
                r#"kind = "password""#,
                r#"kind = "fixed""#,
                &[&STORAGE_ROW.replace("webdav", "ftp")],
            ),
        ),
        (
            "missing field",
            file(
                r#"kind = "password""#,
                r#"kind = "fixed""#,
                &[&STORAGE_ROW.replace(r#", hashes = "none""#, "")],
            ),
        ),
        (
            "an environment variable name in lower case",
            file(
                r#"kind = "agent_login""#,
                r#"kind = "fixed""#,
                &[&AGENT_ROW.replace("ANTHROPIC_API_KEY", "anthropic_api_key")],
            ),
        ),
        (
            "an unknown protocol",
            file(
                r#"kind = "agent_login""#,
                r#"kind = "fixed""#,
                &[&AGENT_ROW.replace("anthropic_messages", "smoke_signals")],
            ),
        ),
        (
            "bad id",
            file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW])
                .replace(r#"id = "example""#, r#"id = "Ex ample""#),
        ),
    ];
    for (name, text) in cases {
        assert!(
            matches!(parse_provider(&text), Err(ProviderFileError::Syntax(_))),
            "{name}"
        );
    }
}

#[test]
fn an_unknown_key_does_not_stop_a_load() {
    let text = file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW])
        .replace("mark =", "homepage = \"x\"\nmark =");
    assert!(parse_provider(&text).is_ok());
}

fn with_face(face: &str) -> String {
    file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW]).replace(
        "mark = \"example\"\n",
        &format!("mark = \"example\"\n\n[mark_face]\n{face}\n"),
    )
}

#[test]
fn a_mark_face_table_gives_the_row_its_letter_and_colour_and_the_word_stays() {
    let spec = parse_provider(&with_face("letter = \"Cx\"\ncolour = \"#d97757\"")).expect("parses");
    assert_eq!(spec.mark, "example");
    let face = spec.mark_face.as_ref().expect("face");
    assert_eq!(
        (face.letter.as_str(), face.colour.as_str()),
        ("Cx", "#D97757")
    );
    let row = spec.sheet_row();
    assert_eq!(row.mark, "example");
    assert_eq!(row.mark_face, spec.mark_face);
}

#[test]
fn a_file_without_a_mark_face_makes_a_row_without_one() {
    let text = file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW]);
    let spec = parse_provider(&text).expect("parses");
    assert_eq!(spec.mark_face, None);
    assert_eq!(spec.sheet_row().mark_face, None);
}

#[test]
fn a_bad_or_half_written_mark_face_refuses_the_file_and_names_the_field() {
    let cases = [
        ("letter = \"ABC\"\ncolour = \"#D97757\"", "letter"),
        ("letter = \"\"\ncolour = \"#D97757\"", "letter"),
        ("letter = \"A B\"\ncolour = \"#D97757\"", "letter"),
        ("letter = \"A\"\ncolour = \"red\"", "colour"),
        ("letter = \"A\"\ncolour = \"#FFF\"", "colour"),
        ("letter = \"A\"\ncolour = \"D97757\"", "colour"),
        ("letter = \"A\"", "colour"),
        ("colour = \"#D97757\"", "letter"),
    ];
    for (face, field) in cases {
        match parse_provider(&with_face(face)) {
            Err(ProviderFileError::Syntax(message)) => {
                assert!(message.contains(field), "{face}: {message}");
                assert!(
                    message.contains("mark_face") || message.contains(field),
                    "{message}"
                );
            }
            other => panic!("{face}: {other:?}"),
        }
    }
}

fn with_group(word: &str, text: String) -> String {
    text.replace(
        "mark = \"example\"\n",
        &format!("mark = \"example\"\ngroup = \"{word}\"\n"),
    )
}

#[test]
fn a_group_word_in_the_file_gives_the_spec_and_the_row_that_group() {
    use porter_core::sheet::ProviderGroup;
    let plain = || file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW]);
    for (word, group) in [
        ("internet", ProviderGroup::Internet),
        ("intelligence", ProviderGroup::Intelligence),
        ("agent", ProviderGroup::Agent),
    ] {
        let spec = parse_provider(&with_group(word, plain())).expect("parses");
        assert_eq!(spec.group, Some(group), "{word}");
        assert_eq!(spec.group(), group, "{word}");
        assert_eq!(spec.sheet_row().group, Some(group), "{word}");
    }
    assert!(matches!(
        parse_provider(&with_group("cloud", plain())),
        Err(ProviderFileError::Syntax(message)) if message.contains("group") || message.contains("cloud")
    ));
}

#[test]
fn a_file_without_a_group_gets_one_from_its_sign_in_and_its_services() {
    use porter_core::sheet::ProviderGroup;
    let agent_login = file(r#"kind = "agent_login""#, r#"kind = "fixed""#, &[AGENT_ROW]);
    let key_ai = file(
        r#"kind = "api_key""#,
        r#"kind = "model_list""#,
        &[AI, LLM_ROW],
    );
    let runtime = file(
        r#"kind = "local_runtime""#,
        r#"kind = "supervised""#,
        &[AI, LLM_ROW],
    );
    let mail_or_files = file(r#"kind = "password""#, r#"kind = "fixed""#, &[STORAGE_ROW]);
    let table = [
        (
            "an agent that signs itself in",
            agent_login,
            ProviderGroup::Agent,
        ),
        (
            "an AI service with a key",
            key_ai,
            ProviderGroup::Intelligence,
        ),
        (
            "a runtime on this computer",
            runtime,
            ProviderGroup::Intelligence,
        ),
        (
            "files with a password",
            mail_or_files,
            ProviderGroup::Internet,
        ),
    ];
    for (name, text, want) in table {
        let spec = parse_provider(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(spec.group, None, "{name}");
        assert_eq!(spec.group(), want, "{name}");
        assert_eq!(spec.sheet_row().group, Some(want), "{name}");
    }
}

fn id() -> porter_core::ProviderId {
    porter_core::ProviderId::parse("example").expect("id")
}

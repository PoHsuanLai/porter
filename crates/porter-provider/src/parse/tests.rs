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

fn id() -> porter_core::ProviderId {
    porter_core::ProviderId::parse("example").expect("id")
}

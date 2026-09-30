use super::*;
use crate::family::Family;
use crate::spec::{Discovery, Port};
use porter_core::capability::{Access, Delta, StorageScope};
use porter_core::{AuthKind, Capability};

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

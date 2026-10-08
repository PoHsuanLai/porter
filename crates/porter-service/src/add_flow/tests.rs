use super::*;
use porter_core::sheet::ProviderKind;

fn row(id: &str, label: &str, kind: RowKind) -> ProviderRow {
    ProviderRow {
        id: ProviderId::parse(id).expect("id"),
        label: label.to_owned(),
        mark: "generic".to_owned(),
        kind,
        auth: ProviderKind::Service,
        mark_face: None,
        group: None,
    }
}

fn labels(rows: Vec<ProviderRow>) -> Vec<String> {
    rows.into_iter().map(|r| r.label).collect()
}

#[test]
fn the_add_list_leaves_out_the_jmap_file_but_the_file_stays_loaded() {
    let all = porter_provider::shipped_specs();
    assert!(all.iter().any(|s| s.id.as_str() == "generic-jmap"));
    let listed: Vec<_> = all.iter().filter(|s| signable(s)).collect();
    assert!(listed.iter().all(|s| s.id.as_str() != "generic-jmap"));
    assert!(listed.iter().any(|s| s.id.as_str() == "generic-imap"));
}

#[test]
fn rows_are_ordered_by_label_not_id_with_the_generic_rows_last_and_mail_first_of_them() {
    // The order the provider files load in is by id: "acp-agent" first, "google-ai" before "google".
    let loaded = vec![
        row("acp-agent", "Other assistant", RowKind::Provider),
        row("anthropic", "Anthropic", RowKind::Provider),
        row(
            "generic-dav",
            "Other calendar or contacts server",
            RowKind::Generic,
        ),
        row("generic-imap", "Other mail account", RowKind::Generic),
        row("generic-jmap", "Other JMAP server", RowKind::Generic),
        row("google-ai", "Google (Gemini)", RowKind::Provider),
        row("google", "Google", RowKind::Provider),
        row("icloud", "iCloud", RowKind::Provider),
        row("fastmail", "fastmail", RowKind::Provider),
    ];
    assert_eq!(
        labels(ordered(loaded)),
        [
            "Anthropic",
            "fastmail",
            "Google",
            "Google (Gemini)",
            "iCloud",
            "Other assistant",
            "Other mail account",
            "Other calendar or contacts server",
            "Other JMAP server",
        ]
    );
}

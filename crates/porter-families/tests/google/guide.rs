//! docs/google.md is what the owner follows in Google's console, so it must say what the code
//! asks: every scope, with the class the code gives it, and the clients.toml row it reads.

use porter_core::capability::CapabilityKind as K;
use porter_families::{Sensitivity, scope_sensitivity, scopes_of};
use porter_oauth::{AppReview, ClientRegistry, MailRights};
use porter_provider::{ClientChannel, Issuer};

fn guide() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/google.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_scope_the_code_asks_is_in_the_guide_with_its_class() {
    let text = guide();
    for kind in [
        K::Mail,
        K::Calendar,
        K::Contacts,
        K::Tasks,
        K::Storage,
        K::Photos,
    ] {
        for scope in scopes_of(kind) {
            let row = text
                .lines()
                .find(|line| line.starts_with('|') && line.contains(&format!("`{scope}`")))
                .unwrap_or_else(|| panic!("{scope} is not in a table row of the guide"));
            let class = match scope_sensitivity(scope) {
                Sensitivity::NonSensitive => "non-sensitive",
                Sensitivity::Sensitive => "sensitive",
                Sensitivity::Restricted => "restricted",
            };
            assert!(row.contains(class), "{scope}: {row}");
            // `sensitive` is also inside `non-sensitive`, so the strongest word must match.
            if class == "sensitive" {
                assert!(
                    !row.contains("non-sensitive") && !row.contains("restricted"),
                    "{row}"
                );
            }
        }
    }
    for always in [
        "openid",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
    ] {
        assert!(text.contains(&format!("`{always}`")), "{always}");
    }
}

#[test]
fn the_row_in_the_guide_is_a_row_the_registry_reads() {
    let text = guide();
    let block = text
        .split("```toml")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("a toml block");
    // The commented `# byo = true` is the owner's to switch on.
    let dir = std::env::temp_dir().join(format!("porter-guide-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (shipped, own) = (dir.join("shipped.toml"), dir.join("own.toml"));
    std::fs::write(&shipped, "").expect("write");
    std::fs::write(&own, block).expect("write");
    let registry = ClientRegistry::from_paths(&shipped, &own).expect("the row parses");
    let client = registry
        .lookup(Issuer::Google, ClientChannel::Stable)
        .expect("the row is google on stable");
    assert_eq!(client.client_id.0, "123-abc.apps.googleusercontent.com");
    assert!(client.client_secret.is_some());
    let traits = registry.traits(Issuer::Google, ClientChannel::Stable);
    assert_eq!(traits.review, AppReview::Testing);
    assert_eq!(traits.mail, MailRights::Withheld);
    let with_mail = block.replace("# byo = true", "byo = true");
    std::fs::write(&own, with_mail).expect("write");
    let registry = ClientRegistry::from_paths(&shipped, &own).expect("the row parses");
    assert_eq!(
        registry.traits(Issuer::Google, ClientChannel::Stable).mail,
        MailRights::Byo
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

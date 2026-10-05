//! Which audiences a grant covers: an audience is the slug of a protocol family, or the fixed
//! endpoint a family's row names (`https://graph.microsoft.com`), and a grant covers those of
//! the families that serve its kind in the provider file. A mail grant also covers `smtp` when
//! the account sends, because SMTP submission has no row of its own.

use porter_core::capability::{Capability, CapabilityKind, Offered};
use porter_core::{Audience, Family};
use porter_provider::ProviderSpec;

/// Whether a grant for `kind` on an account of `spec` covers `audience`.
pub(crate) fn covers(spec: &ProviderSpec, kind: CapabilityKind, audience: &Audience) -> bool {
    spec.capabilities
        .iter()
        .filter(|row| row.capability.kind() == kind)
        .any(|row| {
            let sends =
                matches!(&row.capability, Capability::Mail(mail) if mail.send == Offered::Present);
            audience.0 == row.family.slug()
                || row.endpoint.as_ref().is_some_and(|e| e.0 == audience.0)
                || (sends && audience.0 == Family::Smtp.slug())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_provider::parse_provider;

    const FILE: &str = r#"
id = "mixed"
label = "Mixed"
mark = "generic"

[auth]
kind = "oauth_pkce"
issuer = "microsoft"

[discovery]
kind = "autoconfig"

[[capability]]
family = "imap"
kind = "mail"
v = { access = "read_write", send = "present", delta = "poll", transport = "imap", labels = "folders" }

[[capability]]
family = "graph"
endpoint = "https://graph.microsoft.com"
kind = "calendar"
v = { access = "read_write", delta = "poll", transport = "graph", collections = "present" }

[[capability]]
family = "webdav"
kind = "storage"
v = { access = "read_write", delta = "poll", quota = "reported", scope = "full", hashes = "none", ranges = "present", chunked_upload = "absent" }
"#;

    #[test]
    fn a_grant_covers_the_audiences_of_its_own_kind_only() {
        let spec = parse_provider(FILE).expect("parses");
        const CASES: &[(&str, CapabilityKind, &str, bool)] = &[
            ("mail over imap", CapabilityKind::Mail, "imap", true),
            ("mail sends over smtp", CapabilityKind::Mail, "smtp", true),
            ("mail is not files", CapabilityKind::Mail, "webdav", false),
            ("files over webdav", CapabilityKind::Storage, "webdav", true),
            (
                "files do not send mail",
                CapabilityKind::Storage,
                "smtp",
                false,
            ),
            (
                "calendar by family",
                CapabilityKind::Calendar,
                "graph",
                true,
            ),
            (
                "calendar by resource",
                CapabilityKind::Calendar,
                "https://graph.microsoft.com",
                true,
            ),
            (
                "calendar not by another resource",
                CapabilityKind::Calendar,
                "https://graph.evil.test",
                false,
            ),
            (
                "a kind the provider lacks",
                CapabilityKind::Contacts,
                "graph",
                false,
            ),
            ("a lookalike family", CapabilityKind::Storage, "web", false),
        ];
        for (name, kind, audience, covered) in CASES {
            assert_eq!(
                covers(&spec, *kind, &Audience((*audience).into())),
                *covered,
                "{name}"
            );
        }
    }
}

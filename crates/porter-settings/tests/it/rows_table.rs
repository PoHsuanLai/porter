//! Every key path and spec the accounts module serves, over a registry with a key account, two
//! assistants, two identities and permits of every scope, written out as text and compared with
//! `rows.golden`, one line per key with its fields (path, section, label, help, kind, default,
//! exposure, word labels) and one line per path read back. A change to a row's path, label, help,
//! kind, default, exposure or value, or to the order rows are listed in, shows here. The text is
//! this file's own format, not the `Debug` of quire's types. A mismatch prints the first lines that
//! differ and writes the new text to `rows.golden.new` under the target directory.

use ds_settings::schema::{ActionWeight, Exposure, KeyKind, KeySpec};
use porter_capability::{
    AbsentReason, AgentCap, AgentProgram, AgentProtocol, Capability, CapabilityKind, Claim,
    EnvName, Offer, Provenance, StorageScope, Subject,
};
use porter_core::consent::{Decision, Grant, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountState, AppId, AppName, AuthKind, DataClass, GrantId, GrantKey,
    Isolation, ProviderId, SessionToken, SpaceScope, TokenLifetime,
};
use porter_fake::{llm_account, mail_account, storage_account};
use porter_ids::UnixSeconds;
use porter_service_core::registry::Registry;
use porter_settings::accounts::{Directory, UsedBy, parse, path, read, schema};
use porter_sheet::ProviderGroup;
use std::fmt::Write as _;

/// Knows Fastmail's name and titles every app in capitals, so a title is told from an id.
struct Titled;

impl Directory for Titled {
    fn provider_label(&self, provider: &ProviderId) -> Option<&str> {
        (provider.as_str() == "fastmail").then_some("Fastmail")
    }

    fn group_of(&self, account: &Account) -> ProviderGroup {
        let kinds: Vec<_> = account
            .capabilities
            .iter()
            .map(|c| c.offer.kind())
            .collect();
        ProviderGroup::fallback(account.auth, &kinds)
    }

    fn app_title(&self, app: &AppName) -> String {
        app.as_str().to_uppercase()
    }
}

fn id(text: &str) -> AccountId {
    AccountId::parse(text).expect("id")
}

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("name")
}

fn agent(account: &str, state: AccountState) -> Account {
    let mut agent = storage_account();
    agent.id = id(account);
    agent.auth = AuthKind::AgentLogin;
    agent.state = state;
    agent.capabilities = vec![Claim {
        subject: Subject::Agent(AgentProgram::parse("claude-code").expect("program")),
        offer: Offer::Present(Capability::Agent(Box::new(AgentCap {
            program: AgentProgram::parse("claude-code").expect("program"),
            key_env: Some(EnvName::parse("ANTHROPIC_API_KEY").expect("env")),
            base_url_env: None,
            protocols: [AgentProtocol::AnthropicMessages].into(),
        }))),
        provenance: Provenance::Declared,
    }];
    agent
}

fn grant(
    name: &str,
    account: &AccountId,
    app_name: &str,
    kind: CapabilityKind,
    decision: Decision,
    scope: GrantScope,
    at: i64,
) -> Grant {
    Grant {
        id: GrantId::parse(name).expect("id"),
        key: GrantKey {
            app: AppId {
                name: app(app_name),
                isolation: Isolation::Unsandboxed,
            },
            account: account.clone(),
            kind,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
            reach: StorageScope::AppFolder,
        },
        decision,
        scope,
        at: UnixSeconds(at),
    }
}

fn world() -> (Registry, UsedBy) {
    let mut key = llm_account();
    key.id = id("a-key");
    key.auth = AuthKind::ApiKey;
    // A switch the person turned off still has its row; one for a kind that is not AI has none.
    key.capabilities.push(Claim {
        subject: Subject::Account,
        offer: Offer::Absent {
            kind: CapabilityKind::Embeddings,
            reason: AbsentReason::TurnedOff,
        },
        provenance: Provenance::Declared,
    });
    key.capabilities.push(Claim {
        subject: Subject::Account,
        offer: Offer::Absent {
            kind: CapabilityKind::Speech,
            reason: AbsentReason::NotOnServer,
        },
        provenance: Provenance::Declared,
    });
    let signed_in = agent("an-agent", AccountState::Ok);
    let signed_out = agent("an-agent-out", AccountState::NeedsLogin);
    let mut local = llm_account();
    local.id = id("a-runtime");
    local.auth = AuthKind::LocalRuntime;
    let mut tailscale = storage_account();
    tailscale.id = id("a-tailscale");
    tailscale.auth = AuthKind::OwnProgram;
    tailscale.state = AccountState::NeedsLogin;
    let mut expiring = mail_account();
    expiring.provider = ProviderId::parse("fastmail").expect("id");
    expiring.restriction.token_lifetime = TokenLifetime::SevenDays;
    expiring.restriction.signed_in = Some(UnixSeconds(1_700_000_000));
    let mut expired = storage_account();
    expired.state = AccountState::NeedsReauth;
    expired.restriction.token_lifetime = TokenLifetime::SevenDays;
    expired.restriction.signed_in = Some(UnixSeconds(1_700_000_000));
    let grants = vec![
        grant(
            "g.1",
            &key.id,
            "org.quire.Companion",
            CapabilityKind::Llm,
            Decision::Allow,
            GrantScope::Always,
            0,
        ),
        grant(
            "g2",
            &key.id,
            "org.example.Mail",
            CapabilityKind::Llm,
            Decision::Deny,
            GrantScope::Once,
            1_790_086_400,
        ),
        grant(
            "g3",
            &signed_in.id,
            "org.quire.Companion",
            CapabilityKind::Agent,
            Decision::Allow,
            GrantScope::Session(SessionToken::parse("sess-7").expect("token")),
            86_400,
        ),
    ];
    let mut used = UsedBy::new();
    used.insert(
        expiring.id.clone(),
        vec![app("org.example.Mail"), app("org.quire.Photos")],
    );
    let accounts = vec![
        key, signed_in, signed_out, local, tailscale, expiring, expired,
    ];
    (
        Registry {
            accounts,
            grants,
            toggles: vec![],
        },
        used,
    )
}

/// Paths that are not in the schema, with what `parse` and `read` make of them.
const PROBES: &[&str] = &[
    "accounts.a-key.service.llm",
    "accounts.a-key.service.embeddings",
    "accounts.a-key.service.speech",
    "accounts.a-key.service.storage",
    "accounts.a-key.service.nonsense",
    "accounts.a-key.service.",
    "accounts.a-key.grant.g.1",
    "accounts.a-key.grant.g.1.since",
    "accounts.a-key.grant.g.1.scope",
    "accounts.a-key.grant.g.1.app",
    "accounts.a-key.grant.g.1.service",
    "accounts.a-key.grant.g.1.nonsense",
    "accounts.a-key.grant.g.2",
    "accounts.a-key.grant.g2.since",
    "accounts.a-key.grant.g3",
    "accounts.a-key.grant.",
    "accounts.a-key.grant",
    "accounts.a-key.sign_out",
    "accounts.an-agent.sign_out",
    "accounts.an-agent-out.sign_out",
    "accounts.an-agent.reauth",
    "accounts.an-agent-out.reauth",
    "accounts.a-runtime.reauth",
    "accounts.a-tailscale.reauth",
    "accounts.a-tailscale.remove",
    "accounts.fake-mail.expires",
    "accounts.fake-storage.expires",
    "accounts.a-key.expires",
    "accounts.fake-mail.used_by.0",
    "accounts.fake-mail.used_by.1",
    "accounts.fake-mail.used_by.2",
    "accounts.fake-mail.used_by.x",
    "accounts.fake-mail.used_by.",
    "accounts.fake-mail.used_by",
    "accounts.a-key.used_by.0",
    "accounts.a-key.label",
    "accounts.a-key.nonsense",
    "accounts.a-key",
    "accounts.a-key.",
    "accounts.nobody.state",
    "accounts.clients.google",
    "accounts.clients.google.secret",
    "accounts.clients.microsoft",
    "accounts.clients.microsoft.secret",
    "accounts.clients.openrouter",
    "accounts.",
    "accounts",
    "dock.size",
];

fn line(out: &mut String, registry: &Registry, used: &UsedBy, text: &str) {
    let written = match parse(text, registry) {
        None => writeln!(out, "{text} => no key"),
        Some(key) => writeln!(
            out,
            "{text} => key {} => {}",
            path(&key),
            read(&key, registry, used, &Titled)
                .map_or_else(|| "none".to_owned(), |v| v.to_string())
        ),
    };
    written.expect("write");
}

/// `spec` as one line of its fields.
fn render(spec: &KeySpec) -> String {
    let kind = match &spec.kind {
        KeyKind::Fixed { variant } => format!("fixed {variant}"),
        KeyKind::Toggle { variants } => format!("toggle {}/{}", variants[0], variants[1]),
        KeyKind::Text => "text".to_owned(),
        KeyKind::Live { action } => {
            let weight = match action.weight == ActionWeight::Destructive {
                true => "destructive",
                false => "plain",
            };
            format!("action {} {weight}", action.label.0)
        }
        other => format!("{other:?}"),
    };
    let exposure = match spec.exposure == Exposure::Advanced {
        true => "advanced",
        false => "basic",
    };
    let labels: Vec<String> = spec
        .labels
        .0
        .iter()
        .map(|(word, label)| format!("{word}={label}"))
        .collect();
    format!(
        "{} | {} | {} | {} | {kind} | {} | {exposure} | {}",
        spec.path.0,
        spec.section.0,
        spec.label.0,
        spec.help.0,
        spec.default,
        labels.join(", ")
    )
}

fn table() -> String {
    let (registry, used) = world();
    let schema = schema(&registry, &used, &Titled);
    assert!(schema.check().is_ok());
    let mut out = String::new();
    for spec in &schema.key {
        writeln!(out, "{}", render(spec)).expect("write");
    }
    writeln!(out, "-- every listed path read back").expect("write");
    for spec in &schema.key {
        line(&mut out, &registry, &used, &spec.path.0);
    }
    writeln!(out, "-- probes").expect("write");
    for probe in PROBES {
        line(&mut out, &registry, &used, probe);
    }
    out
}

#[test]
fn the_rows_the_accounts_module_serves_are_the_golden_rows() {
    let actual = table();
    let golden = include_str!("rows.golden");
    if actual != golden {
        let new = concat!(env!("CARGO_TARGET_TMPDIR"), "/rows.golden.new");
        let _ = std::fs::write(new, &actual);
        let differing: Vec<String> = actual
            .lines()
            .zip(golden.lines())
            .enumerate()
            .filter(|(_, (new, old))| new != old)
            .take(5)
            .map(|(n, (new, old))| format!("line {}:\n  now:    {new}\n  golden: {old}", n + 1))
            .collect();
        panic!(
            "rows.golden differs ({} lines now, {} in the golden); first differences:\n{}",
            actual.lines().count(),
            golden.lines().count(),
            differing.join("\n")
        );
    }
}

use super::*;
use crate::catalog::{CatalogDirs, parse_entry_text, read_catalog};
use crate::entries;
use porter_core::capability::{Capability, LlmFeature};
use porter_core::consent::GrantScope;
use std::path::PathBuf;

fn catalogue() -> Vec<ModelEntry> {
    [entries::claude(), entries::luna(), entries::kimi()]
        .iter()
        .map(|text| parse_entry_text(text).expect("a hosted entry parses"))
        .collect()
}

fn granted(grant: &str) -> Verdict {
    Verdict::Granted {
        grant: GrantId::parse(grant).expect("id"),
        scope: GrantScope::Always,
    }
}

fn account(id: &str, verdict: Verdict) -> AccountVerdict {
    AccountVerdict {
        account: AccountId::parse(id).expect("id"),
        provider: None,
        label: None,
        provider_label: None,
        state: None,
        verdict,
    }
}

/// Entry id, account, provider and model id at the provider, for each card.
fn reached(models: &[RemoteModel]) -> Vec<(String, String, String, String)> {
    models
        .iter()
        .map(|m| {
            (
                m.entry.id.0.clone(),
                m.card.account.to_string(),
                m.reach.provider.0.clone(),
                m.reach.model.0.clone(),
            )
        })
        .collect()
}

fn row(
    entry: &str,
    account: &str,
    provider: &str,
    model: &str,
) -> (String, String, String, String) {
    (entry.into(), account.into(), provider.into(), model.into())
}

#[test]
fn a_gateway_account_reaches_every_entry_and_names_the_models_id_there() {
    let models = remote_models(&catalogue(), &[account("openrouter", granted("g-or"))]);
    assert_eq!(
        reached(&models),
        vec![
            row(
                "claude-opus-5.5",
                "openrouter",
                "openrouter",
                "anthropic/claude-opus-5.5"
            ),
            row(
                "gpt-6-luna",
                "openrouter",
                "openrouter",
                "openai/gpt-6-luna"
            ),
            row("kimi-k3", "openrouter", "openrouter", "moonshotai/kimi-k3"),
        ]
    );
    assert!(
        models
            .iter()
            .all(|m| m.grant().is_some_and(|g| g.as_str() == "g-or"))
    );
}

#[test]
fn a_companys_own_account_beats_the_gateway_and_reaches_only_its_own_models() {
    let both = [
        account("openrouter", granted("g-or")),
        account("openai", granted("g-oa")),
    ];
    let models = remote_models(&catalogue(), &both);
    assert_eq!(
        reached(&models),
        vec![
            row(
                "claude-opus-5.5",
                "openrouter",
                "openrouter",
                "anthropic/claude-opus-5.5"
            ),
            row("gpt-6-luna", "openai", "openai", "gpt-6-luna"),
            row("kimi-k3", "openrouter", "openrouter", "moonshotai/kimi-k3"),
        ]
    );
    let only_company = remote_models(&catalogue(), &[account("openai", granted("g-oa"))]);
    assert_eq!(
        reached(&only_company),
        vec![row("gpt-6-luna", "openai", "openai", "gpt-6-luna")]
    );
}

#[test]
fn anthropics_own_account_reaches_nothing_until_a_messages_adapter_exists() {
    let models = remote_models(&catalogue(), &[account("anthropic", granted("g-an"))]);
    assert_eq!(reached(&models), vec![]);
}

#[test]
fn an_account_that_asks_or_is_denied_makes_a_card_that_is_not_granted() {
    let accounts = [
        account("openai", Verdict::Ask),
        account("moonshot", Verdict::Denied),
    ];
    let models = remote_models(&catalogue(), &accounts);
    let verdicts: Vec<_> = models
        .iter()
        .map(|m| (m.entry.id.0.as_str(), m.verdict.clone()))
        .collect();
    assert_eq!(
        verdicts,
        vec![("gpt-6-luna", Verdict::Ask), ("kimi-k3", Verdict::Denied)]
    );
    assert!(models.iter().all(|m| m.grant().is_none()));
}

#[test]
fn a_grant_on_the_gateway_wins_over_an_account_that_only_asks() {
    let accounts = [
        account("openai", Verdict::Ask),
        account("openrouter", granted("g-or")),
    ];
    let models = remote_models(&catalogue(), &accounts);
    let luna = models
        .iter()
        .find(|m| m.entry.id.0 == "gpt-6-luna")
        .expect("luna");
    assert_eq!(luna.card.account.as_str(), "openrouter");
    assert!(luna.grant().is_some());
}

#[test]
fn no_account_is_no_card() {
    assert_eq!(reached(&remote_models(&catalogue(), &[])), vec![]);
}

#[test]
fn a_card_is_a_cloud_model_at_the_reachs_price_that_can_chat_and_call_tools() {
    let models = remote_models(&catalogue(), &[account("openai", granted("g"))]);
    let card = &models[0].card;
    assert_eq!(card.model.as_str(), "gpt-6-luna");
    assert_eq!(card.locality, Where::Cloud { region: None });
    assert_eq!(
        card.billing,
        Billing::Metered(PriceTable {
            input_per_mtok: MicroUsd(100_000),
            output_per_mtok: MicroUsd(500_000),
        })
    );
    let [Capability::Llm(llm)] = card.capabilities.as_slice() else {
        panic!("one language capability: {:?}", card.capabilities);
    };
    assert!(llm.features.contains(&LlmFeature::Tools));
    assert!(llm.features.contains(&LlmFeature::Vision));
}

#[test]
fn a_provider_is_what_accountd_says_else_the_stem_of_the_account_id() {
    let known = known_providers(&catalogue());
    let said = AccountVerdict {
        provider: Some("moonshot".into()),
        ..account("work-kimi", Verdict::Ask)
    };
    let cases = [
        (account("openrouter", Verdict::Ask), Some("openrouter")),
        (account("openrouter-2", Verdict::Ask), Some("openrouter")),
        (account("openai-work", Verdict::Ask), Some("openai")),
        (account("openaix", Verdict::Ask), None),
        (account("local", Verdict::Ask), None),
        (said, Some("moonshot")),
    ];
    for (one, want) in cases {
        let got = provider_of(&one, &known);
        assert_eq!(
            got.as_ref().map(|p| p.0.as_str()),
            want,
            "{:?}",
            one.account
        );
    }
}

#[test]
fn the_longest_provider_stem_wins() {
    let known: BTreeSet<ProviderId> = ["google", "google-ai"].map(|p| ProviderId(p.into())).into();
    let got = provider_of(&account("google-ai-2", Verdict::Ask), &known);
    assert_eq!(got, Some(ProviderId("google-ai".into())));
}

#[test]
fn every_shipped_hosted_entry_is_reached_through_openrouter_and_through_its_company() {
    let catalog = read_catalog(&CatalogDirs {
        system: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stoker/catalog"),
        user: PathBuf::from("/nonexistent/user/catalog"),
    });
    let hosted = remote_entries(&catalog.entries);
    let mut ids: Vec<&str> = hosted.iter().map(|e| e.id.0.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "claude-haiku-4.5",
            "claude-opus-5.5",
            "gemini-3.1-pro",
            "gemini-3.8-flash",
            "gpt-6-astra",
            "gpt-6-luna",
            "kimi-k2.6",
            "kimi-k3"
        ]
    );
    let via_gateway = remote_models(&hosted, &[account("openrouter", granted("g"))]);
    assert_eq!(via_gateway.len(), 8);
    assert!(via_gateway.iter().all(|m| m.reach.is_via_gateway()));
    let companies = [
        account("openai", granted("g1")),
        account("moonshot", granted("g2")),
        account("google-ai", granted("g3")),
    ];
    let direct = remote_models(&hosted, &companies);
    let mut served: Vec<&str> = direct.iter().map(|m| m.entry.id.0.as_str()).collect();
    served.sort_unstable();
    assert_eq!(
        served,
        [
            "gemini-3.1-pro",
            "gemini-3.8-flash",
            "gpt-6-astra",
            "gpt-6-luna",
            "kimi-k2.6",
            "kimi-k3"
        ],
        "Claude waits for the Messages adapter"
    );
}

use super::*;
use crate::catalog::parse_entry_text;
use crate::entries;
use porter_core::AccountId;
use porter_core::consent::Verdict;
use porter_infer::Slot;

fn catalogue() -> Vec<ModelEntry> {
    [
        entries::claude(),
        entries::luna(),
        entries::kimi(),
        entries::hosted(
            "claude-haiku-4.5",
            "Claude Haiku 4.5",
            "claude",
            &[(
                "openrouter",
                "anthropic/claude-haiku-4.5",
                "open_ai_compat",
                1,
                5,
            )],
        ),
    ]
    .iter()
    .map(|text| parse_entry_text(text).expect("parses"))
    .collect()
}

fn account(id: &str) -> AccountVerdict {
    AccountVerdict {
        account: AccountId::parse(id).expect("id"),
        provider: None,
        verdict: Verdict::Ask,
    }
}

#[test]
fn every_hosted_model_is_listed_grouped_by_company_whatever_the_accounts() {
    let listed = choices(&catalogue(), &[], Slot::Text);
    let values: Vec<&str> = listed.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(
        values,
        [
            "cloud/claude-opus-5.5",
            "cloud/claude-haiku-4.5",
            "cloud/gpt-6-luna",
            "cloud/kimi-k3"
        ],
        "a company's models together, companies in the catalogue's order"
    );
    let companies: Vec<&str> = listed.iter().map(|c| c.company.as_str()).collect();
    assert_eq!(companies, ["Anthropic", "Anthropic", "OpenAI", "Moonshot"]);
    assert!(
        listed
            .iter()
            .all(|c| c.available == Availability::NeedsAccount)
    );
}

#[test]
fn a_model_no_account_reaches_says_to_add_one_and_the_others_do_not() {
    let listed = choices(&catalogue(), &[account("openai")], Slot::Text);
    let by_value = |value: &str| listed.iter().find(|c| c.value == value).expect(value);
    let luna = by_value("cloud/gpt-6-luna");
    assert_eq!(luna.available, Availability::Reachable);
    assert!(!luna.label.contains(NEEDS_ACCOUNT), "{}", luna.label);
    let kimi = by_value("cloud/kimi-k3");
    assert_eq!(kimi.available, Availability::NeedsAccount);
    assert!(kimi.label.ends_with(NEEDS_ACCOUNT), "{}", kimi.label);
}

#[test]
fn an_openrouter_account_reaches_all_of_them() {
    let listed = choices(&catalogue(), &[account("openrouter")], Slot::Text);
    assert!(
        listed
            .iter()
            .all(|c| c.available == Availability::Reachable)
    );
}

#[test]
fn a_label_carries_the_company_and_the_capabilities_a_filter_reads() {
    let listed = choices(&catalogue(), &[], Slot::Text);
    assert_eq!(
        listed[0].label,
        "Claude Opus 5.5 (Anthropic, claude, tools, images, reasoning) - Add an account to use"
    );
}

#[test]
fn a_family_the_table_does_not_know_is_capitalised() {
    assert_eq!(company_of("claude"), "Anthropic");
    assert_eq!(company_of("mistral"), "Mistral");
    assert_eq!(company_of(""), "");
}

#[test]
fn an_on_device_entry_is_not_a_hosted_choice() {
    let mut all = catalogue();
    all.push(parse_entry_text(&entries::chat()).expect("local entry"));
    assert_eq!(choices(&all, &[], Slot::Text).len(), 4);
}

#[test]
fn each_slot_lists_the_hosted_models_that_fit_it() {
    let all = catalogue();
    let count = |slot| choices(&all, &[], slot).len();
    assert_eq!(count(Slot::Text), 4);
    assert_eq!(count(Slot::ImageIn), 4, "every curated entry reads images");
    for none in [
        Slot::VoiceIn,
        Slot::VoiceOut,
        Slot::ComputerUse,
        Slot::Embeddings,
        Slot::ImageGen,
        Slot::Rerank,
    ] {
        assert_eq!(count(none), 0, "{none:?}");
    }
}

use super::*;
use model_provider::Money;

fn account(id: &str, provider: &str, label: &str, network: bool) -> AiAccountRow {
    AiAccountRow::new(
        AccountId::parse(id).expect("id"),
        provider.to_owned(),
        None,
        label.to_owned(),
        network,
    )
}

#[derive(Debug)]
struct Fixed(Option<Money>, ChargeTotal);

impl SpendBook for Fixed {
    fn month(&self, _: &AccountId) -> (ChargeTotal, Option<Money>) {
        (self.1, self.0)
    }
}

fn rows(accounts: Vec<AiAccountRow>) -> SpendRows {
    let list = AccountList::default();
    list.replace(accounts);
    SpendRows::new(list, Arc::new(Fixed(None, ChargeTotal::default())))
}

#[test]
fn only_an_account_over_the_network_has_spend_rows() {
    let rows = rows(vec![
        account("hosted", "openrouter", "Work", true),
        account("ollama", "ollama", "Ollama", false),
    ]);
    let paths: Vec<String> = rows.schema().into_iter().map(|row| row.path.0).collect();
    assert_eq!(
        paths,
        ["accounts.hosted.spend.month", "accounts.hosted.spend.cap"]
    );
    assert!(rows.handles("accounts.hosted.spend.cap"));
    assert!(
        !rows.handles("accounts.ollama.spend.cap"),
        "nothing to spend"
    );
    assert!(!rows.handles("accounts.hosted.state"), "accountd's row");
    assert!(!rows.handles("accounts.nobody.spend.cap"));
}

/// Both modules spell one account's rows the same way: the id in a spend row's path is found by
/// the function accountd's rows are found by, and the paths this module lists are the ones
/// `porter-settings` would parse, dotted ids included.
#[test]
fn the_spend_rows_name_the_same_account_as_accountds_rows_for_an_id_with_a_dot() {
    let ids = ["a.b", "a", "openrouter-2"];
    let rows = rows(ids.map(|id| account(id, "openrouter", id, true)).to_vec());
    let listed = rows.schema();
    assert_eq!(listed.len(), 6);
    let accounts: Vec<AccountId> = ids
        .iter()
        .map(|id| AccountId::parse(id).expect("id"))
        .collect();
    for spec in &listed {
        let (found, tail) =
            account_tail(&spec.path.0, accounts.iter()).expect("an account of the list");
        assert!(tail == SPEND_MONTH || tail == SPEND_CAP, "{tail}");
        assert!(ids.contains(&found.as_str()));
        // The section is the account's label, as accountd's rows put the account's block.
        assert_eq!(spec.section.0, found.as_str());
    }
    // The longest id wins: `a.b`'s rows are not `a`'s.
    assert!(rows.handles("accounts.a.b.spend.month"));
}

#[tokio::test]
async fn the_rows_read_the_ledger_in_whole_cents_rounded_up() {
    let list = AccountList::default();
    list.replace(vec![account("hosted", "openrouter", "Work", true)]);
    let usd = |millionths: i64| Money::usd(Decimal::new(millionths, 6));
    let rows = SpendRows::new(
        list,
        Arc::new(Fixed(Some(usd(5_000_000)), ChargeTotal::default())),
    );
    assert_eq!(
        rows.get("accounts.hosted.spend.cap").await.expect("cap"),
        toml::Value::String("500".into())
    );
    assert_eq!(
        rows.get("accounts.hosted.spend.month")
            .await
            .expect("month"),
        toml::Value::String("0".into())
    );
    assert!(matches!(
        rows.set("accounts.hosted.spend.cap", toml::Value::Boolean(true))
            .await,
        Err(LiveError::NotPermitted(_))
    ));
    assert!(matches!(
        rows.get("accounts.hosted.state").await,
        Err(LiveError::UnknownKey(_))
    ));
}

#[test]
fn spend_is_whole_cents_and_a_part_of_a_cent_counts_as_one() {
    let usd = |millionths: i64| Money::usd(Decimal::new(millionths, 6));
    assert_eq!(cents_up(usd(0)), "0");
    assert_eq!(cents_up(usd(1)), "1");
    assert_eq!(cents_up(usd(10_000)), "1");
    assert_eq!(cents_up(usd(1_234_567)), "124");
    let euros = Money::new(Decimal::new(3, 0), Currency::EUR);
    assert_eq!(
        cents_up(euros),
        "3 EUR",
        "another currency is written as it is"
    );
}

/// Before any request runs, the limit row of an account whose provider reports no charges says
/// the limit cannot apply; OpenRouter, which reports each reply's charge, says nothing of the sort.
#[test]
fn the_limit_row_says_when_the_provider_reports_no_charges() {
    let rows = rows(vec![
        account("sees-nothing", "anthropic", "Anthropic", true),
        account("sees-all", "openrouter", "OpenRouter", true),
    ]);
    let listed = rows.schema();
    let help = |path: &str| {
        listed
            .iter()
            .find(|row| row.path.0 == path)
            .unwrap_or_else(|| panic!("{path}"))
            .help
            .0
            .clone()
    };
    assert_eq!(
        help("accounts.sees-nothing.spend.cap"),
        "anthropic doesn't report what its replies cost, so a limit can't apply to it."
    );
    assert!(help("accounts.sees-nothing.spend.month").contains("nothing is counted"));
    assert_eq!(help("accounts.sees-all.spend.cap"), "");
    assert_eq!(help("accounts.sees-all.spend.month"), "");
}

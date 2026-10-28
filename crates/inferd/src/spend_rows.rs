//! What each AI account has spent on hosted models this month, and the monthly cap on it, as rows
//! of inferd's settings module: `accounts.<id>.spend.month` ("Spent this month") and
//! `accounts.<id>.spend.cap` ("Monthly limit"), one pair for each account reached over the
//! network (a program on this computer costs nothing).
//!
//! The account itself, and every other row of it, is accountd's: Settings shows one block per
//! account and reads these two beside accountd's own rows. Both modules find the account a path
//! names with the same function ([`porter_settings::accounts::account_tail`]), so an id with a dot
//! in it is the same account in both. The rows read inferd's ledger, which is why they stay here.

use crate::broker::AccountList;
use crate::cloud::accountd::Boxed;
use crate::cloud::spend::written;
use crate::cloud::wire::charges_of;
use ds_settings::live::LiveError;
use ds_settings::schema::{Help, KeySpec};
use iso_currency::Currency;
use model_provider::{Charges, Money};
use porter_core::{AccountId, AiAccountRow};
use porter_infer::ChargeTotal;
use porter_settings::accounts::{SPEND_CAP, SPEND_MONTH, account_tail};
use rust_decimal::Decimal;
use std::sync::Arc;

/// What accounts have spent on hosted models this month, and the cap on each.
pub trait SpendBook: Send + Sync + 'static {
    /// What was spent through `account` this calendar month (UTC), and the monthly cap on an
    /// account (`None`: no cap).
    fn month(&self, account: &AccountId) -> (ChargeTotal, Option<Money>);
}

/// `money` as the spend rows show it: US dollars in whole cents, as digits, rounded up so a part
/// of a cent shows as one; an amount in another currency as its digits and code (`3 EUR`).
fn cents_up(money: Money) -> String {
    if money.currency != Currency::USD {
        return written(money);
    }
    money
        .amount
        .checked_mul(Decimal::ONE_HUNDRED)
        .map_or_else(|| written(money), |cents| cents.ceil().to_string())
}

/// The rows over the accounts accountd lists and the ledger of `spend`.
#[derive(Clone)]
pub struct SpendRows {
    accounts: AccountList,
    spend: Arc<dyn SpendBook>,
}

impl std::fmt::Debug for SpendRows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpendRows").finish_non_exhaustive()
    }
}

/// Which of the two rows a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Month,
    Cap,
}

impl SpendRows {
    /// The rows of `accounts`, spend read from `spend`.
    pub fn new(accounts: AccountList, spend: Arc<dyn SpendBook>) -> Self {
        Self { accounts, spend }
    }

    fn spec(account: &AiAccountRow, row: Row) -> KeySpec {
        // Only what a provider reports is counted. For one that reports nothing, both rows say so
        // before any request runs: its replies show tokens and never dollars, so a limit on it
        // could never be met.
        let reported = charges_of(&model_catalog::ProviderId(account.provider.clone()));
        let name = account
            .provider_label
            .as_deref()
            .unwrap_or(&account.provider);
        let says = |what: &str| format!("{name} doesn't report what its replies cost, so {what}.");
        let (suffix, label, help) = match (row, reported) {
            (Row::Month, Charges::PerReply) => (SPEND_MONTH, "Spent this month", String::new()),
            (Row::Cap, Charges::PerReply) => (SPEND_CAP, "Monthly limit", String::new()),
            (Row::Month, _) => (
                SPEND_MONTH,
                "Spent this month",
                says("nothing is counted here"),
            ),
            (Row::Cap, _) => (
                SPEND_CAP,
                "Monthly limit",
                says("a limit can't apply to it"),
            ),
        };
        let mut spec = porter_settings::readout(
            format!("accounts.{}.{suffix}", account.account),
            &account.label,
            label,
            "0",
            &[],
        );
        spec.help = Help(help);
        spec
    }

    /// Every row, for the schema: a pair for each account reached over the network.
    pub fn schema(&self) -> Vec<KeySpec> {
        self.accounts
            .now()
            .iter()
            .filter(|account| account.network)
            .flat_map(|account| {
                [
                    Self::spec(account, Row::Month),
                    Self::spec(account, Row::Cap),
                ]
            })
            .collect()
    }

    fn row_of(&self, path: &str) -> Option<(AccountId, Row)> {
        let now = self.accounts.now();
        let (id, tail) = account_tail(path, now.iter().map(|a| &a.account))?;
        let row = match tail {
            SPEND_MONTH => Row::Month,
            SPEND_CAP => Row::Cap,
            _ => return None,
        };
        now.iter()
            .any(|a| a.account == id && a.network)
            .then_some((id, row))
    }

    /// Whether `path` is one of these rows.
    pub fn handles(&self, path: &str) -> bool {
        self.row_of(path).is_some()
    }

    /// The value of a row.
    pub fn get<'a>(&'a self, path: &'a str) -> Boxed<'a, Result<toml::Value, LiveError>> {
        Box::pin(async move {
            let (id, row) = self
                .row_of(path)
                .ok_or_else(|| LiveError::UnknownKey(path.to_owned()))?;
            let (total, cap) = self.spend.month(&id);
            let text = match row {
                Row::Month => cents_up(total.reported),
                Row::Cap => cap.map_or_else(|| "0".to_owned(), cents_up),
            };
            Ok(toml::Value::String(text))
        })
    }

    /// A row is a read-out.
    pub fn set<'a>(
        &'a self,
        path: &'a str,
        _value: toml::Value,
    ) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(async move {
            match self.handles(path) {
                true => Err(LiveError::NotPermitted("this row is a read-out".into())),
                false => Err(LiveError::UnknownKey(path.to_owned())),
            }
        })
    }
}

#[cfg(test)]
mod tests;

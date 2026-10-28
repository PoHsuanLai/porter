//! Spend caps (design/31 §5.5): per account and per app, daily and monthly, warn then stop. A cap
//! compares the charges the providers reported, never an estimate: it can stop the next turn and
//! never the current one.

use model_provider::Money;
use porter_core::{AccountId, AppId};
use porter_ids::{Permille, TurnCount};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Who a cap limits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SpendScope {
    /// Everything spent through one account.
    Account(AccountId),
    /// Everything one app spends.
    App(AppId),
}

/// The window a cap counts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    /// A calendar day, local time.
    Daily,
    /// A calendar month, local time.
    Monthly,
}

/// One cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendCap {
    /// Who it limits.
    pub scope: SpendScope,
    /// Over which window.
    pub period: Period,
    /// The limit, in the currency it is set in.
    pub limit: Money,
    /// The share of the limit that warns (setting `ai.spend.warn_permille`, proposed 800).
    pub warn_at: Permille,
}

/// What a cap says about one more request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendVerdict {
    /// Under the warning line.
    Within,
    /// Past the warning line, under the limit: run, and say so.
    Warn,
    /// It would reach the limit: stop and show the reason.
    Stop,
}

/// What a scope has been charged: the sum of the charges providers reported, and how many turns
/// reported none. Only a reported charge ever counts toward a cap; a turn with none is counted
/// here and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChargeTotal {
    /// The sum of the reported charges, in the currency of the first one (US dollars, zero, when
    /// none has come).
    pub reported: Money,
    /// The turns that had no charge to add.
    pub unpriced: TurnCount,
}

impl Default for ChargeTotal {
    fn default() -> Self {
        Self {
            reported: Money::usd(Decimal::ZERO),
            unpriced: TurnCount(0),
        }
    }
}

impl ChargeTotal {
    /// The total after one more turn that carried `charge`. A turn with none is `unpriced`, and so
    /// is one whose charge cannot be added: another currency than the total's, or a sum too large
    /// to hold. Nothing is converted, guessed or dropped.
    pub fn counting(self, charge: Option<Money>) -> Self {
        let unpriced = Self {
            unpriced: TurnCount(self.unpriced.0.saturating_add(1)),
            ..self
        };
        let Some(charge) = charge else {
            return unpriced;
        };
        // A total that is still zero has no currency yet: the first charge gives it one.
        let sum = if self.reported.amount.is_zero() {
            Ok(charge)
        } else {
            self.reported.checked_add(charge)
        };
        match sum {
            Ok(reported) => Self { reported, ..self },
            Err(_) => unpriced,
        }
    }
}

/// What a cap says about one more request, given what has been reported so far. A cap in another
/// currency than the reported total is never met, and a turn that reported nothing never meets
/// one.
pub fn spend_verdict(cap: &SpendCap, spent: &ChargeTotal) -> SpendVerdict {
    if spent.reported.currency != cap.limit.currency {
        return SpendVerdict::Within;
    }
    let (spent, limit) = (spent.reported.amount, cap.limit.amount);
    if spent >= limit {
        return SpendVerdict::Stop;
    }
    // A warning line too large to hold is no line: only the limit speaks.
    let warn_line = limit
        .checked_mul(Decimal::from(cap.warn_at.0))
        .and_then(|scaled| scaled.checked_div(Decimal::ONE_THOUSAND))
        .unwrap_or(limit);
    if spent >= warn_line {
        SpendVerdict::Warn
    } else {
        SpendVerdict::Within
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iso_currency::Currency;
    use porter_core::AccountId;

    fn usd(cents: i64) -> Money {
        Money::usd(Decimal::new(cents, 2))
    }

    fn total(cents: i64) -> ChargeTotal {
        ChargeTotal {
            reported: usd(cents),
            unpriced: TurnCount(0),
        }
    }

    fn cap(limit_cents: i64) -> SpendCap {
        SpendCap {
            scope: SpendScope::Account(AccountId::parse("openrouter").expect("id")),
            period: Period::Daily,
            limit: usd(limit_cents),
            warn_at: Permille(800),
        }
    }

    #[test]
    fn caps_warn_at_the_line_and_stop_at_the_limit() {
        let cases = [
            ("well under", 100, 10, SpendVerdict::Within),
            ("just under the line", 100, 79, SpendVerdict::Within),
            ("on the line", 100, 80, SpendVerdict::Warn),
            ("just under the limit", 100, 99, SpendVerdict::Warn),
            ("at the limit", 100, 100, SpendVerdict::Stop),
            ("already over", 100, 150, SpendVerdict::Stop),
        ];
        for (name, limit, spent, expected) in cases {
            let got = spend_verdict(&cap(limit), &total(spent));
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn turns_that_reported_nothing_never_meet_a_cap() {
        let mut spent = ChargeTotal::default();
        for _ in 0..1_000 {
            spent = spent.counting(None);
        }
        assert_eq!(spent.unpriced, TurnCount(1_000));
        assert_eq!(spend_verdict(&cap(1), &spent), SpendVerdict::Within);
    }

    #[test]
    fn a_cap_in_another_currency_than_the_reports_is_never_met() {
        let euros = SpendCap {
            limit: Money::new(Decimal::ONE, Currency::EUR),
            ..cap(1)
        };
        assert_eq!(
            spend_verdict(&euros, &total(1_000_000)),
            SpendVerdict::Within
        );
    }

    #[test]
    fn reported_charges_add_exactly_and_the_rest_are_counted_unpriced() {
        let first = ChargeTotal::default().counting(Some(Money::usd(Decimal::new(1, 1))));
        let both = first.counting(Some(Money::usd(Decimal::new(2, 1))));
        assert_eq!(both.reported, Money::usd(Decimal::new(3, 1)));
        assert_eq!(both.unpriced, TurnCount(0));

        // The first charge gives an empty total its currency.
        let euros = Money::new(Decimal::new(5, 0), Currency::EUR);
        assert_eq!(ChargeTotal::default().counting(Some(euros)).reported, euros);

        // A charge in another currency than the total's is not added, and not lost: it is counted.
        let mixed = both.counting(Some(euros));
        assert_eq!(mixed.reported, both.reported);
        assert_eq!(mixed.unpriced, TurnCount(1));

        // So is a sum too large to hold.
        let top = ChargeTotal {
            reported: Money::usd(Decimal::MAX),
            unpriced: TurnCount(0),
        };
        let over = top.counting(Some(Money::usd(Decimal::ONE)));
        assert_eq!((over.reported, over.unpriced), (top.reported, TurnCount(1)));
    }
}

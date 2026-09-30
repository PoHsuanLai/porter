//! Spend caps (design/31 §5.5): per account and per app, daily and monthly, warn then stop.

use crate::reply::TokenUsage;
use porter_core::{AccountId, AppId, MicroUsd, Permille, PriceTable};
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpendCap {
    /// Who it limits.
    pub scope: SpendScope,
    /// Over which window.
    pub period: Period,
    /// The limit.
    pub limit: MicroUsd,
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

/// What `usage` costs at `price`, rounded up to a whole micro-dollar.
pub fn cost(usage: TokenUsage, price: &PriceTable) -> MicroUsd {
    let part =
        |tokens: u32, per_mtok: MicroUsd| (u64::from(tokens) * per_mtok.0).div_ceil(1_000_000);
    MicroUsd(
        part(usage.input.0, price.input_per_mtok) + part(usage.output.0, price.output_per_mtok),
    )
}

/// The verdict for spending `estimate` more after `spent` this period.
pub fn spend_verdict(cap: &SpendCap, spent: MicroUsd, estimate: MicroUsd) -> SpendVerdict {
    let after = spent.0.saturating_add(estimate.0);
    let warn_line = cap.limit.0.saturating_mul(u64::from(cap.warn_at.0)) / 1000;
    if after >= cap.limit.0 {
        SpendVerdict::Stop
    } else if after >= warn_line {
        SpendVerdict::Warn
    } else {
        SpendVerdict::Within
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AccountId, Tokens};

    fn cap(limit: u64) -> SpendCap {
        SpendCap {
            scope: SpendScope::Account(AccountId::parse("openrouter").expect("id")),
            period: Period::Daily,
            limit: MicroUsd(limit),
            warn_at: Permille(800),
        }
    }

    #[test]
    fn caps_warn_at_the_line_and_stop_at_the_limit() {
        let cases = [
            ("well under", 100, 0, 10, SpendVerdict::Within),
            ("just under the line", 100, 70, 9, SpendVerdict::Within),
            ("on the line", 100, 70, 10, SpendVerdict::Warn),
            ("just under the limit", 100, 90, 9, SpendVerdict::Warn),
            ("reaching the limit", 100, 90, 10, SpendVerdict::Stop),
            ("already over", 100, 150, 0, SpendVerdict::Stop),
        ];
        for (name, limit, spent, estimate, expected) in cases {
            let got = spend_verdict(&cap(limit), MicroUsd(spent), MicroUsd(estimate));
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn cost_is_per_million_tokens_rounded_up() {
        let price = PriceTable {
            input_per_mtok: MicroUsd(3_000_000),
            output_per_mtok: MicroUsd(15_000_000),
        };
        let cases = [
            ("nothing", 0, 0, 0),
            ("a million in", 1_000_000, 0, 3_000_000),
            ("one token each way", 1, 1, 3 + 15),
        ];
        for (name, input, output, expected) in cases {
            let usage = TokenUsage {
                input: Tokens(input),
                output: Tokens(output),
            };
            assert_eq!(cost(usage, &price), MicroUsd(expected), "{name}");
        }
        let cheap = PriceTable {
            input_per_mtok: MicroUsd(1),
            output_per_mtok: MicroUsd(0),
        };
        let usage = TokenUsage {
            input: Tokens(1),
            output: Tokens(0),
        };
        assert_eq!(
            cost(usage, &cheap),
            MicroUsd(1),
            "a fraction rounds up to one"
        );
    }
}

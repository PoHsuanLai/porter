//! What hosted models have cost: a ledger of micro-dollars and tokens per scope (an account, an
//! app) and period (a calendar day, a calendar month, in UTC), and the verdict the caps
//! (`ai.spend.*`) give one more request.
//!
//! The meter counts what a reply's usage says at the price of the reach used (`porter_infer::cost`,
//! rounded up to a micro-dollar). It keeps only the current day and month, in memory and, when it
//! was opened on a file, in that file (a small JSON document, written whole through a rename, mode
//! 0600) so a restart does not reset a cap. The file holds money and counts, never a key or a
//! prompt.

use crate::settings::SpendLine;
use porter_core::{AccountId, AppId, MicroUsd, Tokens, UnixSeconds};
use porter_infer::{Period, SpendScope, SpendVerdict, TokenUsage, spend_verdict};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// What one scope spent in one period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spent {
    /// Micro-dollars.
    pub micro_usd: u64,
    /// Prompt tokens.
    pub tokens_in: u64,
    /// Reply tokens.
    pub tokens_out: u64,
}

/// The days since 1970-01-01 of `at` (UTC).
fn day_of(at: UnixSeconds) -> i64 {
    at.0.div_euclid(86_400)
}

/// The month `at` is in, as months since year 0 (UTC): `year * 12 + month - 1`.
fn month_of(at: UnixSeconds) -> i64 {
    // Howard Hinnant's civil-from-days: the proleptic Gregorian calendar.
    let z = day_of(at) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    year * 12 + month - 1
}

/// The index of the period `at` falls in.
fn bucket_of(period: Period, at: UnixSeconds) -> i64 {
    match period {
        Period::Daily => day_of(at),
        Period::Monthly => month_of(at),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Key {
    scope: SpendScope,
    period: Period,
    bucket: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Row {
    #[serde(flatten)]
    key: Key,
    spent: Spent,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    rows: Vec<Row>,
}

/// Every scope's spend this day and month.
#[derive(Debug, Default)]
pub struct Ledger {
    rows: Mutex<HashMap<Key, Spent>>,
    file: Option<PathBuf>,
}

impl Ledger {
    /// A ledger that remembers nothing across a restart.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// A ledger kept in `file`: what it holds is read now (a file that is missing or does not
    /// read starts it empty) and every record is written back.
    pub fn open(file: PathBuf) -> Self {
        let rows = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<Document>(&text).ok())
            .map(|document| {
                document
                    .rows
                    .into_iter()
                    .map(|r| (r.key, r.spent))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            rows: Mutex::new(rows),
            file: Some(file),
        }
    }

    /// What `scope` has spent in the period `now` is in.
    pub fn spent(&self, scope: &SpendScope, period: Period, now: UnixSeconds) -> Spent {
        let key = Key {
            scope: scope.clone(),
            period,
            bucket: bucket_of(period, now),
        };
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .copied()
            .unwrap_or_default()
    }

    /// Counts one reply: `cost` and `usage` against the app and the account, in the day and the
    /// month `now` is in; periods that have ended are forgotten.
    pub fn record(
        &self,
        app: &AppId,
        account: &AccountId,
        usage: TokenUsage,
        cost: MicroUsd,
        now: UnixSeconds,
    ) {
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        rows.retain(|key, _| key.bucket >= bucket_of(key.period, now));
        let scopes = [
            SpendScope::App(app.clone()),
            SpendScope::Account(account.clone()),
        ];
        for scope in scopes {
            for period in [Period::Daily, Period::Monthly] {
                let spent = rows
                    .entry(Key {
                        scope: scope.clone(),
                        period,
                        bucket: bucket_of(period, now),
                    })
                    .or_default();
                spent.micro_usd = spent.micro_usd.saturating_add(cost.0);
                spent.tokens_in = spent.tokens_in.saturating_add(u64::from(usage.input.0));
                spent.tokens_out = spent.tokens_out.saturating_add(u64::from(usage.output.0));
            }
        }
        self.save(&rows);
    }

    fn save(&self, rows: &HashMap<Key, Spent>) {
        let Some(file) = &self.file else { return };
        let document = Document {
            rows: rows
                .iter()
                .map(|(key, spent)| Row {
                    key: key.clone(),
                    spent: *spent,
                })
                .collect(),
        };
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let temp = file.with_extension("json.new");
            let text = serde_json::to_string(&document).map_err(std::io::Error::other)?;
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temp)?;
            out.write_all(text.as_bytes())?;
            std::fs::rename(&temp, file)
        };
        if let Err(why) = write() {
            eprintln!("inferd: spend: {}: {why}", file.display());
        }
    }

    /// What the caps say about one more request by `app` through `account` that would cost about
    /// `estimate`: the strictest of the caps that bear on it (`Within` when none is set).
    pub fn verdict(
        &self,
        line: SpendLine,
        app: &AppId,
        account: &AccountId,
        estimate: MicroUsd,
        now: UnixSeconds,
    ) -> SpendVerdict {
        line.caps_for(app, account)
            .iter()
            .map(|cap| {
                let spent = self.spent(&cap.scope, cap.period, now).micro_usd;
                spend_verdict(cap, MicroUsd(spent), estimate)
            })
            .fold(SpendVerdict::Within, strictest)
    }

    /// What `app` used today and this month, by name (the `Usage` dictionary of `Inference1`):
    /// micro-dollars and tokens for the day, micro-dollars for the month, and each cap that is set.
    pub fn usage_of(&self, line: SpendLine, app: &AppId, now: UnixSeconds) -> Vec<(String, u64)> {
        let scope = SpendScope::App(app.clone());
        let day = self.spent(&scope, Period::Daily, now);
        let month = self.spent(&scope, Period::Monthly, now);
        let mut rows = vec![
            ("tokens_in_day".to_owned(), day.tokens_in),
            ("tokens_out_day".to_owned(), day.tokens_out),
            ("spend_day_micro_usd".to_owned(), day.micro_usd),
            ("spend_month_micro_usd".to_owned(), month.micro_usd),
        ];
        let caps = line.limits.app;
        rows.extend(
            [
                ("cap_day_micro_usd", caps.daily),
                ("cap_month_micro_usd", caps.monthly),
            ]
            .into_iter()
            .filter_map(|(name, limit)| Some((name.to_owned(), limit?.0))),
        );
        rows
    }
}

fn strictest(a: SpendVerdict, b: SpendVerdict) -> SpendVerdict {
    let rank = |verdict: SpendVerdict| match verdict {
        SpendVerdict::Within => 0,
        SpendVerdict::Warn => 1,
        SpendVerdict::Stop => 2,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// The cost of a turn whose size is not known yet, for the check before it starts: a thousand
/// tokens each way at the reach's price.
pub fn estimate(price: &porter_core::PriceTable) -> MicroUsd {
    porter_infer::cost(
        TokenUsage {
            input: Tokens(1_000),
            output: Tokens(1_000),
            cached: Tokens(0),
        },
        price,
    )
}

#[cfg(test)]
mod tests;

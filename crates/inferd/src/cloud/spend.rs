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
use porter_capability::PriceTable;
use porter_core::AccountId;
use porter_core::AppId;
use porter_fs::atomic::AtomicWrite;
use porter_ids::{MicroUsd, Tokens, UnixSeconds};
use porter_infer::{Period, SpendScope, SpendVerdict, TokenUsage, cost, spend_verdict};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

/// The month `at` is in, as months since year 0 (UTC): `year * 12 + month - 1`. An instant chrono
/// cannot write (beyond year 262000) is month 0.
fn month_of(at: UnixSeconds) -> i64 {
    use chrono::Datelike;
    chrono::DateTime::from_timestamp(at.0, 0).map_or(0, |day| {
        i64::from(day.year()) * 12 + i64::from(day.month()) - 1
    })
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
    /// Set when the ledger's file could not be read: the day and month it was found in, whose
    /// spend is not known (kept in the file so a restart does not forget it).
    #[serde(default)]
    unknown: Option<Unknown>,
}

/// The day and the month in which the ledger lost what had been spent. A cap of that period
/// stops everything until the period ends, since what was spent cannot be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Unknown {
    day: i64,
    month: i64,
}

impl Spent {
    fn of(usage: TokenUsage, cost: MicroUsd) -> Self {
        Self {
            micro_usd: cost.0,
            tokens_in: u64::from(usage.input.0),
            tokens_out: u64::from(usage.output.0),
        }
    }
}

impl Unknown {
    fn at(now: UnixSeconds) -> Self {
        Self {
            day: day_of(now),
            month: month_of(now),
        }
    }

    /// Whether the period `now` is in is the one that was lost.
    fn covers(self, period: Period, now: UnixSeconds) -> bool {
        match period {
            Period::Daily => self.day == day_of(now),
            Period::Monthly => self.month == month_of(now),
        }
    }
}

/// Every scope's spend this day and month.
#[derive(Debug, Default)]
pub struct Ledger {
    rows: Mutex<HashMap<Key, Spent>>,
    file: Option<PathBuf>,
    unknown: Option<Unknown>,
}

impl Ledger {
    /// A ledger that remembers nothing across a restart.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// A ledger kept in `file`: what it holds is read now and every record is written back. A
    /// file that is missing starts it empty. A file that is there and cannot be read is not
    /// quietly forgotten (that would reset every cap to nothing spent): it is moved aside as
    /// `<name>.corrupt-<seconds>`, and the day and month `now` is in count as unknown, so a cap
    /// of either stops requests until that period ends.
    pub fn open_at(file: PathBuf, now: UnixSeconds) -> Self {
        let read = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str::<Document>(&text).map_err(|_| ()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Document::default()),
            Err(_) => Err(()),
        };
        let (document, lost) = match read {
            Ok(document) => (document, false),
            Err(()) => {
                set_aside(&file, now);
                (Document::default(), true)
            }
        };
        // A loss from an earlier run counts while its period lasts.
        let unknown = match lost {
            true => Some(Unknown::at(now)),
            false => document.unknown.filter(|u| {
                [Period::Daily, Period::Monthly]
                    .into_iter()
                    .any(|period| u.covers(period, now))
            }),
        };
        let rows = document
            .rows
            .into_iter()
            .map(|r| (r.key, r.spent))
            .collect();
        let ledger = Self {
            rows: Mutex::new(rows),
            file: Some(file),
            unknown,
        };
        if lost {
            // Keep the loss in the file at once: a restart before the first record must not
            // forget it.
            ledger.save(&ledger.rows.lock().unwrap_or_else(PoisonError::into_inner));
        }
        ledger
    }

    /// Whether what was spent in the period `now` is in is lost.
    fn is_unknown(&self, period: Period, now: UnixSeconds) -> bool {
        self.unknown.is_some_and(|u| u.covers(period, now))
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
        self.swap(
            &mut rows,
            app,
            account,
            Spent::default(),
            Spent::of(usage, cost),
            now,
        );
    }

    /// Takes `held` off the app's and the account's day and month and puts `counted` on, then
    /// writes the ledger. Periods that have ended are forgotten.
    fn swap(
        &self,
        rows: &mut HashMap<Key, Spent>,
        app: &AppId,
        account: &AccountId,
        held: Spent,
        counted: Spent,
        now: UnixSeconds,
    ) {
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
                spent.micro_usd = spent.micro_usd.saturating_sub(held.micro_usd);
                spent.tokens_in = spent.tokens_in.saturating_sub(held.tokens_in);
                spent.tokens_out = spent.tokens_out.saturating_sub(held.tokens_out);
                spent.micro_usd = spent.micro_usd.saturating_add(counted.micro_usd);
                spent.tokens_in = spent.tokens_in.saturating_add(counted.tokens_in);
                spent.tokens_out = spent.tokens_out.saturating_add(counted.tokens_out);
            }
        }
        self.save(rows);
    }

    /// Sets aside the worst a turn may cost, if the caps allow it: the check and the hold are
    /// one step under the ledger's lock, so turns that start together see each other's holds and
    /// cannot together pass a cap. `None` when a cap stops it.
    ///
    /// The hold counts as spent until the guard is settled, and stays counted if the guard is
    /// dropped without settling (an error, a client that went away, a task that was aborted).
    pub fn reserve<'a>(
        &'a self,
        line: SpendLine,
        app: &AppId,
        account: &AccountId,
        price: &PriceTable,
        worst: TokenUsage,
        now: UnixSeconds,
    ) -> Option<SpendGuard<'a>> {
        let held = Spent::of(worst, cost(worst, price));
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        let stop = self.verdict_in(&rows, line, app, account, MicroUsd(held.micro_usd), now)
            == SpendVerdict::Stop;
        if stop {
            return None;
        }
        self.swap(&mut rows, app, account, Spent::default(), held, now);
        Some(SpendGuard {
            ledger: self,
            app: app.clone(),
            account: account.clone(),
            price: *price,
            held,
            at: now,
        })
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
            unknown: self.unknown,
        };
        // On disk before it replaces the old file, or a power cut leaves an empty ledger.
        let write = || -> std::io::Result<()> {
            let text = serde_json::to_string(&document).map_err(std::io::Error::other)?;
            AtomicWrite::PRIVATE.write(file, text.as_bytes())
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
        let rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        self.verdict_in(&rows, line, app, account, estimate, now)
    }

    /// [`Ledger::verdict`] over rows the caller already holds the lock on.
    fn verdict_in(
        &self,
        rows: &HashMap<Key, Spent>,
        line: SpendLine,
        app: &AppId,
        account: &AccountId,
        estimate: MicroUsd,
        now: UnixSeconds,
    ) -> SpendVerdict {
        line.caps_for(app, account)
            .iter()
            .map(|cap| match self.is_unknown(cap.period, now) {
                // What was spent this period is lost: a cap cannot be told to hold, so it stops.
                true => SpendVerdict::Stop,
                false => {
                    let key = Key {
                        scope: cap.scope.clone(),
                        period: cap.period,
                        bucket: bucket_of(cap.period, now),
                    };
                    let spent = rows.get(&key).copied().unwrap_or_default().micro_usd;
                    spend_verdict(cap, MicroUsd(spent), estimate)
                }
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

/// Moves a ledger file that could not be read out of the way, to `<name>.corrupt-<seconds>`, so
/// it can be looked at and the next save does not write over it.
fn set_aside(file: &std::path::Path, now: UnixSeconds) {
    let name = file.file_name().map_or_else(
        || "spend.json".into(),
        |name| name.to_string_lossy().into_owned(),
    );
    let aside = file.with_file_name(format!("{name}.corrupt-{}", now.0));
    match std::fs::rename(file, &aside) {
        Ok(()) => eprintln!(
            "inferd: spend: {} could not be read; moved to {}. Spending limits stop requests \
             until the day and the month end.",
            file.display(),
            aside.display()
        ),
        Err(why) => eprintln!("inferd: spend: {}: {why}", file.display()),
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

/// The worst a hosted turn can use, for the hold before it runs: the prompt at one token to three
/// bytes (more tokens than text usually has) plus the longest reply the turn may ask for.
pub fn worst_case(prompt_bytes: usize, max_output: Tokens) -> TokenUsage {
    TokenUsage {
        input: Tokens(u32::try_from(prompt_bytes.div_ceil(3)).unwrap_or(u32::MAX)),
        output: max_output,
        cached: Tokens(0),
    }
}

/// A hold on the ledger for one turn that may bill, taken by [`Ledger::reserve`].
///
/// Settle it with what the turn used. A guard that is dropped without settling (the turn failed,
/// the client went away, the task was aborted) leaves the whole hold counted, since the provider
/// may have billed for it all.
#[derive(Debug)]
pub struct SpendGuard<'a> {
    ledger: &'a Ledger,
    app: AppId,
    account: AccountId,
    price: PriceTable,
    held: Spent,
    at: UnixSeconds,
}

impl SpendGuard<'_> {
    /// Replaces the hold with what the turn used. `None` is a turn whose usage is not known (the
    /// model may have run and no count came, as the provider's report of how the attempt ended
    /// says): the hold stays as the charge.
    pub fn settle(self, used: Option<TokenUsage>) {
        let Some(used) = used else { return };
        let mut rows = self
            .ledger
            .rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let counted = Spent::of(used, cost(used, &self.price));
        self.ledger.swap(
            &mut rows,
            &self.app,
            &self.account,
            self.held,
            counted,
            self.at,
        );
    }
}

/// The cost of a turn whose size is not known yet, for the check before it starts: a thousand
/// tokens each way at the reach's price.
pub fn estimate(price: &PriceTable) -> MicroUsd {
    cost(
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

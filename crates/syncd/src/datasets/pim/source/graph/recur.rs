//! Graph's `patternedRecurrence` as an iCalendar `RRULE`.
//!
//! Covered: `daily`, `weekly`, `absoluteMonthly`, `relativeMonthly`, `absoluteYearly`,
//! `relativeYearly`, with `noEnd`, `endDate` and `numbered` ranges. A pattern of any other type
//! (or one missing what its type needs) has no rule: the event is written as its series master
//! with a `COMMENT`, and the occurrences are whatever Graph sends.

use super::json::{Pattern, Range, Recurrence};

/// What a recurrence came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// The value of an `RRULE`.
    Rrule(String),
    /// No rule could be made; the text says why, for a `COMMENT`.
    Unsupported(String),
}

fn day(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "sunday" => "SU",
        "monday" => "MO",
        "tuesday" => "TU",
        "wednesday" => "WE",
        "thursday" => "TH",
        "friday" => "FR",
        "saturday" => "SA",
        _ => return None,
    })
}

fn position(index: &str) -> Option<i8> {
    Some(match index.to_ascii_lowercase().as_str() {
        "first" => 1,
        "second" => 2,
        "third" => 3,
        "fourth" => 4,
        "last" => -1,
        _ => return None,
    })
}

fn days(pattern: &Pattern) -> Option<Vec<&'static str>> {
    let all: Option<Vec<_>> = pattern.days_of_week.iter().map(|d| day(d)).collect();
    all.filter(|d| !d.is_empty())
}

/// `20261006` from `2026-10-06` (a date, or the date part of a date-time).
pub(super) fn compact_date(text: &str) -> Option<String> {
    let date = text.get(..10)?;
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_digit());
    (digits(y, 4) && digits(m, 2) && digits(d, 2)).then(|| format!("{y}{m}{d}"))
}

fn end_of(range: Option<&Range>, all_day: bool) -> Option<String> {
    let range = range?;
    match range.kind.as_deref() {
        Some("endDate") => {
            let date = compact_date(range.end_date.as_deref()?)?;
            Some(match all_day {
                true => format!("UNTIL={date}"),
                false => format!("UNTIL={date}T235959Z"),
            })
        }
        Some("numbered") => Some(format!("COUNT={}", range.occurrences.filter(|n| *n > 0)?)),
        _ => None,
    }
}

/// The rule for a recurrence of an event that is (`all_day`) or is not an all-day one.
pub fn rule(recurrence: &Recurrence, all_day: bool) -> Rule {
    let Some(pattern) = recurrence.pattern.as_ref() else {
        return Rule::Unsupported("the recurrence has no pattern".into());
    };
    let kind = pattern.kind.as_deref().unwrap_or("none");
    let interval = pattern.interval.filter(|n| *n > 0).unwrap_or(1);
    let unsupported = || Rule::Unsupported(format!("recurrence pattern {kind} is not mirrored"));
    let mut parts: Vec<String> = Vec::new();
    match kind {
        "daily" => parts.push("FREQ=DAILY".into()),
        "weekly" => {
            let Some(days) = days(pattern) else {
                return unsupported();
            };
            parts.push("FREQ=WEEKLY".into());
            parts.push(format!("INTERVAL={interval}"));
            parts.push(format!("BYDAY={}", days.join(",")));
            if let Some(first) = pattern.first_day_of_week.as_deref().and_then(day) {
                parts.push(format!("WKST={first}"));
            }
        }
        "absoluteMonthly" => {
            let Some(of) = pattern.day_of_month else {
                return unsupported();
            };
            parts.push("FREQ=MONTHLY".into());
            parts.push(format!("INTERVAL={interval}"));
            parts.push(format!("BYMONTHDAY={of}"));
        }
        "relativeMonthly" | "relativeYearly" => {
            let (Some(days), Some(at)) =
                (days(pattern), pattern.index.as_deref().and_then(position))
            else {
                return unsupported();
            };
            match kind {
                "relativeMonthly" => {
                    parts.push("FREQ=MONTHLY".into());
                    parts.push(format!("INTERVAL={interval}"));
                }
                _ => {
                    let Some(month) = pattern.month else {
                        return unsupported();
                    };
                    parts.push("FREQ=YEARLY".into());
                    parts.push(format!("INTERVAL={interval}"));
                    parts.push(format!("BYMONTH={month}"));
                }
            }
            match days.as_slice() {
                [one] => parts.push(format!("BYDAY={at}{one}")),
                many => {
                    parts.push(format!("BYDAY={}", many.join(",")));
                    parts.push(format!("BYSETPOS={at}"));
                }
            }
        }
        "absoluteYearly" => {
            let (Some(month), Some(of)) = (pattern.month, pattern.day_of_month) else {
                return unsupported();
            };
            parts.push("FREQ=YEARLY".into());
            parts.push(format!("INTERVAL={interval}"));
            parts.push(format!("BYMONTH={month}"));
            parts.push(format!("BYMONTHDAY={of}"));
        }
        _ => return unsupported(),
    }
    if kind == "daily" {
        parts.push(format!("INTERVAL={interval}"));
    }
    parts.extend(end_of(recurrence.range.as_ref(), all_day));
    Rule::Rrule(parts.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_of(json: &str, all_day: bool) -> Rule {
        rule(&serde_json::from_str(json).expect("recurrence"), all_day)
    }

    #[test]
    fn each_common_pattern_and_range_is_a_rule() {
        const CASES: &[(&str, &str, bool, &str)] = &[
            (
                "daily, no end",
                r#"{"pattern":{"type":"daily","interval":2},"range":{"type":"noEnd"}}"#,
                false,
                "FREQ=DAILY;INTERVAL=2",
            ),
            (
                "weekly, numbered, week starts Monday",
                r#"{"pattern":{"type":"weekly","interval":1,"daysOfWeek":["monday","wednesday"],"firstDayOfWeek":"monday"},"range":{"type":"numbered","numberOfOccurrences":10}}"#,
                false,
                "FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;WKST=MO;COUNT=10",
            ),
            (
                "absolute monthly, end date, timed",
                r#"{"pattern":{"type":"absoluteMonthly","interval":3,"dayOfMonth":15},"range":{"type":"endDate","endDate":"2027-01-15"}}"#,
                false,
                "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=15;UNTIL=20270115T235959Z",
            ),
            (
                "absolute monthly, end date, all day",
                r#"{"pattern":{"type":"absoluteMonthly","interval":1,"dayOfMonth":1},"range":{"type":"endDate","endDate":"2027-01-01"}}"#,
                true,
                "FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=1;UNTIL=20270101",
            ),
            (
                "relative monthly, one day",
                r#"{"pattern":{"type":"relativeMonthly","interval":1,"daysOfWeek":["tuesday"],"index":"second"},"range":{"type":"noEnd"}}"#,
                false,
                "FREQ=MONTHLY;INTERVAL=1;BYDAY=2TU",
            ),
            (
                "relative monthly, last weekday set",
                r#"{"pattern":{"type":"relativeMonthly","interval":1,"daysOfWeek":["monday","tuesday"],"index":"last"},"range":{"type":"noEnd"}}"#,
                false,
                "FREQ=MONTHLY;INTERVAL=1;BYDAY=MO,TU;BYSETPOS=-1",
            ),
            (
                "absolute yearly",
                r#"{"pattern":{"type":"absoluteYearly","interval":1,"month":10,"dayOfMonth":6},"range":{"type":"noEnd"}}"#,
                true,
                "FREQ=YEARLY;INTERVAL=1;BYMONTH=10;BYMONTHDAY=6",
            ),
            (
                "relative yearly",
                r#"{"pattern":{"type":"relativeYearly","interval":1,"month":11,"daysOfWeek":["thursday"],"index":"fourth"},"range":{"type":"noEnd"}}"#,
                true,
                "FREQ=YEARLY;INTERVAL=1;BYMONTH=11;BYDAY=4TH",
            ),
            (
                "no interval means every one",
                r#"{"pattern":{"type":"daily"},"range":{"type":"noEnd"}}"#,
                false,
                "FREQ=DAILY;INTERVAL=1",
            ),
        ];
        for (name, json, all_day, want) in CASES {
            assert_eq!(
                rule_of(json, *all_day),
                Rule::Rrule((*want).to_owned()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_pattern_it_cannot_say_has_no_rule() {
        const CASES: &[(&str, &str)] = &[
            ("unknown type", r#"{"pattern":{"type":"hourly"}}"#),
            ("weekly without days", r#"{"pattern":{"type":"weekly"}}"#),
            (
                "weekly with a day it does not know",
                r#"{"pattern":{"type":"weekly","daysOfWeek":["someday"]}}"#,
            ),
            (
                "relative without an index",
                r#"{"pattern":{"type":"relativeMonthly","daysOfWeek":["monday"]}}"#,
            ),
            ("no pattern", r#"{}"#),
        ];
        for (name, json) in CASES {
            assert!(
                matches!(rule_of(json, false), Rule::Unsupported(_)),
                "{name}"
            );
        }
    }
}

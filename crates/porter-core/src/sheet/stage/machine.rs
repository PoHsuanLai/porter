//! The transition table of the sheet machine.
//!
//! Ported from the rules of mailo's add-account machine
//! (`mailo/crates/mail-app/src/ui/add_account/flow.rs`, MIT OR Apache-2.0, same author): ask
//! before looking, look once and not again on a second press, add only when told to (Confirm),
//! check a form field by field, say a refusal and keep going, never let what was typed leave
//! except into the one call that needs it. Here the pieces are porter's: a sign-in
//! conversation (`Feed`), not mailo's lookup and add.
//!
//! Rules, in order of importance:
//! - A typed secret appears in exactly one effect, `Feed(Fields)`; nothing else carries it.
//! - `Store` comes only when the sign-in says `Done`: after a `Confirm` it carries the person's
//!   choices (kept in `Stage::Confirming`), after a re-sign-in's `Done` with no review it is empty.
//!   A sign-in that fails after `Confirm` stores nothing.
//! - A second press while something runs does nothing (`Working` takes no input but Dismiss).
//! - Dismiss and `Left` cancel a sign-in that is in flight, then close; every stage can end.
//! - What does not fit the stage (a late progress, a pick on the form) is dropped, not guessed.
//!
//! The consent alert is not a stage: it is one call (`Sheets::consent`), so `Answer` is not
//! this machine's input and is dropped here.

use super::{Purpose, Sheet, SheetEffect, SheetEnd, SheetEvent, Stage};
use crate::sheet::fields::{FieldAnswer, FieldSpec, FieldValue};
use crate::sheet::input::SheetInput;
use crate::sheet::manual;
use crate::sheet::progress::{Progress, SignInFault, SignInInput};
use crate::wire::ProviderHint;

pub(super) fn step(sheet: Sheet, event: SheetEvent) -> (Sheet, Vec<SheetEffect>) {
    match event {
        SheetEvent::Input(SheetInput::Dismiss) | SheetEvent::Left => leave(sheet),
        SheetEvent::Input(input) => on_input(sheet, input),
        SheetEvent::SignIn(progress) => on_progress(sheet, progress),
        SheetEvent::Stored => on_stored(sheet),
        SheetEvent::StoreFailed => on_store_failed(sheet),
    }
}

fn show(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    let effects = vec![SheetEffect::Show(sheet.view())];
    (sheet, effects)
}

fn ignore(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    (sheet, vec![])
}

fn to(sheet: Sheet, stage: Stage) -> (Sheet, Vec<SheetEffect>) {
    show(Sheet { stage, ..sheet })
}

/// Whether a sign-in has begun and not ended, so that leaving must stop it.
fn in_flight(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::Working(_)
            | Stage::Asking { .. }
            | Stage::Browser { .. }
            | Stage::Code { .. }
            | Stage::Confirming { .. }
            | Stage::Reviewing { .. }
    )
}

fn leave(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    let end = match &sheet.stage {
        Stage::Added => return ignore(sheet),
        Stage::Failed { fault, .. } => SheetEnd::Failed(*fault),
        _ => SheetEnd::Dismissed,
    };
    let mut effects = Vec::new();
    if in_flight(&sheet.stage) {
        effects.push(SheetEffect::Feed(SignInInput::Cancel));
    }
    effects.push(SheetEffect::Close(end));
    (sheet, effects)
}

fn on_input(sheet: Sheet, input: SheetInput) -> (Sheet, Vec<SheetEffect>) {
    match (&sheet.stage, input) {
        (Stage::Choosing(rows), SheetInput::Pick(id)) if rows.iter().any(|r| r.id == id) => {
            let mut effects = vec![SheetEffect::Feed(SignInInput::Start)];
            let (sheet, shown) = to(sheet, Stage::Working(id));
            effects.splice(0..0, shown);
            (sheet, effects)
        }
        (Stage::Asking { .. }, SheetInput::Submit(answers)) => submit(sheet, answers),
        (Stage::Reviewing { provider, .. }, SheetInput::Confirm(choices)) => {
            let provider = provider.clone();
            let feed = SheetEffect::Feed(SignInInput::Confirm(choices.clone()));
            let (sheet, mut effects) = to(sheet, Stage::Confirming { provider, choices });
            effects.push(feed);
            (sheet, effects)
        }
        (Stage::Browser { url, .. }, SheetInput::OpenAgain) => {
            let effects = vec![SheetEffect::OpenBrowser(url.clone())];
            (sheet, effects)
        }
        (Stage::Failed { provider, fault }, SheetInput::Retry) if retryable(*fault) => {
            let provider = provider.clone();
            restart(sheet, provider, false)
        }
        (
            Stage::Asking { provider, .. }
            | Stage::Browser { provider, .. }
            | Stage::Code { provider, .. }
            | Stage::Reviewing { provider, .. }
            | Stage::Failed { provider, .. },
            SheetInput::Back,
        ) => {
            let provider = provider.clone();
            let cancel = in_flight(&sheet.stage);
            back(sheet, provider, cancel)
        }
        _ => ignore(sheet),
    }
}

/// One step back: to the provider list where the person chose from it, else the provider's
/// sign-in starts over from its first question.
fn back(sheet: Sheet, provider: crate::id::ProviderId, cancel: bool) -> (Sheet, Vec<SheetEffect>) {
    let listed = matches!(
        sheet.purpose,
        Purpose::Add {
            hint: ProviderHint::Any,
            ..
        }
    ) && !sheet.providers.is_empty();
    if !listed {
        return restart(sheet, provider, cancel);
    }
    let list = Stage::Choosing(sheet.providers.clone());
    let (sheet, mut effects) = to(sheet, list);
    if cancel {
        effects.push(SheetEffect::Feed(SignInInput::Cancel));
    }
    (sheet, effects)
}

/// Starts the provider's sign-in again from its first question.
fn restart(
    sheet: Sheet,
    provider: crate::id::ProviderId,
    cancel: bool,
) -> (Sheet, Vec<SheetEffect>) {
    let (sheet, mut effects) = to(sheet, Stage::Working(provider));
    if cancel {
        effects.push(SheetEffect::Feed(SignInInput::Cancel));
    }
    effects.push(SheetEffect::Feed(SignInInput::Start));
    (sheet, effects)
}

/// A refusal of the organisation or a missing client id does not change by asking again.
fn retryable(fault: SignInFault) -> bool {
    !matches!(fault, SignInFault::Forbidden | SignInFault::NeedsClientId)
}

/// The answers to fields that were asked, in the form's order, plain text trimmed.
fn asked(fields: &[FieldSpec], answers: Vec<FieldAnswer>) -> Vec<FieldAnswer> {
    fields
        .iter()
        .filter_map(|spec| {
            let answer = answers.iter().find(|a| a.kind == spec.kind)?;
            let value = match &answer.value {
                FieldValue::Plain(text) => FieldValue::Plain(text.trim().to_string()),
                secret @ FieldValue::Secret(_) => secret.clone(),
            };
            Some(FieldAnswer {
                kind: spec.kind,
                value,
            })
        })
        .collect()
}

fn submit(sheet: Sheet, answers: Vec<FieldAnswer>) -> (Sheet, Vec<SheetEffect>) {
    let Stage::Asking {
        provider, fields, ..
    } = &sheet.stage
    else {
        return ignore(sheet);
    };
    // The server form changes with its answers (JMAP has no outgoing server): what is checked
    // and sent is the form as the answers make it.
    let fields = &manual::refit(fields, &answers);
    if let Some(problem) = manual::form_problem(fields, &answers) {
        let stage = Stage::Asking {
            provider: provider.clone(),
            fields: fields.clone(),
            problem: Some(problem),
        };
        return to(sheet, stage);
    }
    let feed = SheetEffect::Feed(SignInInput::Fields(asked(fields, answers)));
    let provider = provider.clone();
    let (sheet, mut effects) = to(sheet, Stage::Working(provider));
    effects.push(feed);
    (sheet, effects)
}

fn on_progress(sheet: Sheet, progress: Progress) -> (Sheet, Vec<SheetEffect>) {
    let provider = match &sheet.stage {
        Stage::Working(p)
        | Stage::Confirming { provider: p, .. }
        | Stage::Browser { provider: p, .. }
        | Stage::Code { provider: p, .. } => p.clone(),
        _ => return ignore(sheet),
    };
    match progress {
        Progress::Done => done(sheet),
        Progress::Waiting => (sheet, vec![SheetEffect::Feed(SignInInput::Poll)]),
        Progress::Failed(fault) => to(sheet, Stage::Failed { provider, fault }),
        // A question, a page, a code or a review belongs to a sign-in that has not been
        // confirmed; after `Confirm` they are dropped.
        _ if matches!(sheet.stage, Stage::Confirming { .. }) => ignore(sheet),
        // Signing in again never asks what to use again: the account keeps its services, so a
        // sign-in that offers a review is confirmed at once with no changes.
        Progress::Review(_) if again(&sheet) => {
            let stage = Stage::Confirming {
                provider,
                choices: Vec::new(),
            };
            // Confirming draws what Working did: a sheet already working shows nothing new.
            let (sheet, mut effects) = match sheet.stage {
                Stage::Working(_) => (Sheet { stage, ..sheet }, vec![]),
                _ => to(sheet, stage),
            };
            effects.push(SheetEffect::Feed(SignInInput::Confirm(Vec::new())));
            (sheet, effects)
        }
        Progress::Ask(fields) => to(
            sheet,
            Stage::Asking {
                provider,
                fields,
                problem: None,
            },
        ),
        Progress::Browser(url) => to(sheet, Stage::Browser { provider, url }),
        Progress::Code { user_code, url } => to(
            sheet,
            Stage::Code {
                provider,
                user_code,
                url,
            },
        ),
        Progress::Review(review) => to(sheet, Stage::Reviewing { provider, review }),
    }
}

/// The sign-in is done: the only place an account is stored.
fn done(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    match (&sheet.stage, &sheet.purpose) {
        (Stage::Confirming { choices, .. }, _) => {
            let store = SheetEffect::Store(choices.clone());
            (sheet, vec![store])
        }
        // A sign-in again that needed a page or a code is done while the sheet still shows it.
        (
            Stage::Working(_) | Stage::Browser { .. } | Stage::Code { .. },
            Purpose::Reauthenticate { .. },
        ) => (sheet, vec![SheetEffect::Store(vec![])]),
        _ => ignore(sheet),
    }
}

fn on_stored(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    match sheet.stage {
        Stage::Working(_) | Stage::Confirming { .. } => stored(sheet),
        Stage::Browser { .. } | Stage::Code { .. } if again(&sheet) => stored(sheet),
        _ => ignore(sheet),
    }
}

/// Whether the sheet signs an existing account in again.
fn again(sheet: &Sheet) -> bool {
    matches!(sheet.purpose, Purpose::Reauthenticate { .. })
}

fn stored(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    let (sheet, mut effects) = to(sheet, Stage::Added);
    effects.push(SheetEffect::Close(SheetEnd::Added));
    (sheet, effects)
}

fn on_store_failed(sheet: Sheet) -> (Sheet, Vec<SheetEffect>) {
    let provider = match &sheet.stage {
        Stage::Working(provider) | Stage::Confirming { provider, .. } => provider,
        Stage::Browser { provider, .. } | Stage::Code { provider, .. } if again(&sheet) => provider,
        _ => return ignore(sheet),
    };
    let stage = Stage::Failed {
        provider: provider.clone(),
        fault: SignInFault::StoreFailed,
    };
    to(sheet, stage)
}

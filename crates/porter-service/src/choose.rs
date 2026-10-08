//! `Choose`: the chooser and consent sheet, and the grant it records.

use crate::registry::{Asker, Registry, candidate};
use porter_core::consent::{AccountChoice, ConsentAnswer, ConsentAsk, Decision, Grant, GrantScope};
use porter_core::wire::Refusal;
use porter_core::{AccountId, AccountsReply, GrantId, LauncherSession, Need, UnixSeconds};

/// The sheet to show for `need`, or `None` when no account fits. `session` is the open launcher
/// session the request was made in, which lets the sheet offer "This session only".
pub(crate) fn ask_for(
    registry: &Registry,
    need: &Need,
    asker: Asker<'_>,
    session: Option<&LauncherSession>,
) -> Option<ConsentAsk> {
    let accounts: Vec<AccountChoice> = registry
        .fitting(need)
        .iter()
        .map(|fit| AccountChoice {
            account: fit.account.id.clone(),
            label: fit.account.label.clone(),
            provider: fit.account.provider.clone(),
        })
        .collect();
    (!accounts.is_empty()).then(|| ConsentAsk {
        app: asker.app.clone(),
        kind: need.kind(),
        class: asker.class,
        usage: asker.usage,
        accounts,
        session: session.cloned(),
        app_label: None,
    })
}

/// Records `answer` in `registry` and gives the reply. An `Allow` for an account that was not
/// offered is treated as dismissed, and so is one whose scope is a session other than the one
/// the ask carried (`session`): a sheet cannot make a session scope the ask did not offer.
pub(crate) fn settle(
    registry: &mut Registry,
    need: &Need,
    asker: Asker<'_>,
    session: Option<&LauncherSession>,
    answer: ConsentAnswer,
    at: UnixSeconds,
) -> AccountsReply {
    match answer {
        ConsentAnswer::Allow {
            scope: GrantScope::Session(chosen),
            ..
        } if session != Some(&chosen) => AccountsReply::Refused(Refusal::Dismissed),
        ConsentAnswer::Dismissed => AccountsReply::Refused(Refusal::Dismissed),
        ConsentAnswer::Deny => {
            let keys: Vec<_> = registry
                .fitting(need)
                .iter()
                .map(|fit| asker.key(fit))
                .collect();
            for key in keys {
                let id = next_grant_id(registry);
                registry.grants.push(Grant {
                    id,
                    key,
                    decision: Decision::Deny,
                    scope: GrantScope::Always,
                    at,
                });
            }
            AccountsReply::Refused(Refusal::Denied)
        }
        // The service runs the add sheet for this answer before it settles (`add_and_allow`); a
        // settle that is handed it records nothing.
        ConsentAnswer::AddAccount => AccountsReply::Refused(Refusal::Dismissed),
        ConsentAnswer::Allow { account, scope } => {
            allow(registry, need, asker, &account, scope, at)
        }
    }
}

fn allow(
    registry: &mut Registry,
    need: &Need,
    asker: Asker<'_>,
    account: &AccountId,
    scope: GrantScope,
    at: UnixSeconds,
) -> AccountsReply {
    let id = next_grant_id(registry);
    let fits = registry.fitting(need);
    let Some(fit) = fits.iter().find(|fit| fit.account.id == *account) else {
        return AccountsReply::Refused(Refusal::Dismissed);
    };
    let key = asker.key(fit);
    // No grant is ever recorded over another app's own Space.
    if !key.space_is_open() {
        return AccountsReply::Refused(Refusal::Denied);
    }
    let chosen = candidate(fit, id.clone());
    let grant = Grant {
        id,
        key,
        decision: Decision::Allow,
        scope,
        at,
    };
    registry.grants.push(grant);
    AccountsReply::Chosen(chosen)
}

/// A grant id unused in `registry`.
pub(crate) fn next_grant_id(registry: &Registry) -> GrantId {
    let n = registry.grants.len() + 1;
    let taken = |id: &GrantId| registry.grants.iter().any(|g| g.id == *id);
    (n..)
        .filter_map(|i| GrantId::parse(&format!("grant-{i}")).ok())
        .find(|id| !taken(id))
        .unwrap_or_else(|| unreachable!("the ids are unbounded and grant-<n> is well formed"))
}

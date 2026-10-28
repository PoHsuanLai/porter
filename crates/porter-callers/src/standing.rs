//! A peephole [`Standing`] as a porter [`Caller`]: which app it is, how that was established, and
//! which role the proof reaches. Pure: the table and the standing in, the caller out.

use super::translate::role_of_name;
use super::{Caller, CallerRole, CallerTable};
use peephole_core::{AppKey, Engine, Proof, Standing};
use porter_core::{AppId, AppName, Isolation, PeerIdentity};

/// The app `standing` shows, if any.
///
/// - A sandbox the runtime or kernel attests (`Sandboxed`) is that app, `Flatpak` or `Snap`.
/// - A service a table row matched is the row's app, `Unsandboxed`.
/// - A program in an app scope of a launcher is the scope's app, `Unsandboxed`: the name is a
///   claim, so consent here is advisory, as it always was for a native app. So is one whose image
///   the person can write (a build of their own, `~/.local/bin`): peephole keeps its scope's claim
///   for exactly that case (WAVE2 §2.8).
/// - Every other program and every other `Unverified` process shows nothing.
pub(super) fn identity_of(standing: &Standing) -> PeerIdentity {
    match standing {
        Standing::Service { app, .. } => proven(app, Isolation::Unsandboxed),
        Standing::Sandboxed { app, engine } => match engine {
            Engine::Flatpak => proven(app, Isolation::Flatpak),
            Engine::Snap => proven(app, Isolation::Snap),
            _ => PeerIdentity::Unproven,
        },
        Standing::Program {
            scope_app: Some(app),
            ..
        }
        | Standing::Unverified {
            scope_app: Some(app),
            ..
        } => proven(app, Isolation::Unsandboxed),
        _ => PeerIdentity::Unproven,
    }
}

/// `app` as a proven identity when it is a reverse-DNS app name (a snap's bare name is not one).
fn proven(app: &AppKey, isolation: Isolation) -> PeerIdentity {
    match AppName::parse(app.as_str()) {
        Ok(name) => PeerIdentity::Proven(AppId { name, isolation }),
        Err(_) => PeerIdentity::Unproven,
    }
}

/// The proof a role needs: a fixed role stands for more than an app, so it needs the unit
/// manager's word (`Unit`, or `Full` where the image can be read too); the others need only that
/// the process is the row's.
const fn proof_needed(role: CallerRole) -> Proof {
    match role.is_fixed() {
        true => Proof::Unit,
        false => Proof::Cgroup,
    }
}

/// The role a service standing reaches: the row's role when the proof is enough for it, else a
/// plain app. A role the table does not know (the manager's) is nobody's.
fn service_role(role: &peephole_core::RoleName, proof: Proof) -> Option<CallerRole> {
    let row = role_of_name(role)?;
    Some(match proof >= proof_needed(row) {
        true => row,
        false => CallerRole::App,
    })
}

/// The caller `standing` is, or `None` for nobody.
///
/// A service has its row's role (lowered to `App` when the proof falls short, see
/// [`proof_needed`]). An app, sandboxed or in a launcher's scope, has the role of the table's row
/// for it that names no unit (`App` when there is none): a unit row's role goes to its unit alone.
/// An image the person can write never reaches a fixed role, whatever its scope says: nothing
/// about it may be trusted beyond the claim.
pub(super) fn caller_of(standing: &Standing, table: &CallerTable) -> Option<Caller> {
    let PeerIdentity::Proven(app) = identity_of(standing) else {
        return None;
    };
    let role = match standing {
        Standing::Service { role, proof, .. } => service_role(role, *proof)?,
        Standing::Unverified { .. } => match table.role_of(&app.name) {
            role if role.is_fixed() => CallerRole::App,
            role => role,
        },
        _ => table.role_of(&app.name),
    };
    Some(Caller { app, role })
}

#[cfg(test)]
mod tests;

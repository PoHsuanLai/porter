//! An edit of a mail account's servers (`SignInMode::EditServers`): the servers typed are tried
//! before anything is kept. The incoming server is logged in to with the password typed, or the
//! one the host holds when none was; the outgoing (SMTP) server too, when it, the login name or
//! the password changed; a JMAP session is fetched over https. Nothing is filed: the sign-in
//! ends `Done` with the servers it logged in at and no credential, and the host hands back what
//! was typed.
//!
//! A refusal ends the sign-in with the fault that says what was wrong (a password refused, a
//! server that did not answer); the sheet asks the form again with the field marked and starts a
//! new sign-in. No bearer token is ever sent to a typed host: the only credential here is a
//! password the person or the host gave, or the JMAP token the person typed for that server.

use super::{GenericFlow, Lookup, Mid};
use crate::jmap;
use generic_proto::mail::typed as mail_typed;
use porter_core::{Credential, Family, LoginName, Restriction, SecretText};
use porter_family_kit::Move;
use porter_family_kit::password::typed_password;
use porter_provider::{SignInMode, Signed};
use porter_sheet::{FieldAnswer, Manual, SignInFault, parse_manual};

/// Whether the outgoing server is to be tried: it, or the login name, differs from what the
/// account has, or a new password was typed.
fn outgoing_changed(current: &Manual, typed: &Manual, password: Option<&SecretText>) -> bool {
    let outgoing = |manual: &Manual| match manual {
        Manual::Imap(servers) | Manual::Pop3(servers) => Some(servers.outgoing.clone()),
        _ => None,
    };
    password.is_some() || outgoing(current) != outgoing(typed) || current.login() != typed.login()
}

impl<L: Lookup> GenericFlow<L> {
    /// Tries the servers in `answers` for the account being edited, and ends `Done` with them
    /// only when they let it in.
    pub(super) async fn edit(&self, answers: Vec<FieldAnswer>) -> Move<Mid<L::Mid>> {
        let SignInMode::EditServers {
            label,
            current,
            keep,
            ..
        } = &self.mode
        else {
            return Move::Fail(SignInFault::Unreadable);
        };
        // The sheet has checked the form; one that still does not read, or that names another
        // protocol than the account's, is not this edit.
        let Ok(typed) = parse_manual(&answers) else {
            return Move::Fail(SignInFault::Unreadable);
        };
        if typed.protocol() != current.protocol() {
            return Move::Fail(SignInFault::Unreadable);
        }
        let password = typed_password(&answers);
        let token = match &typed {
            Manual::Jmap(server) => server.token.clone(),
            _ => None,
        };
        let incoming = match (&token, &password, keep) {
            (Some(token), _, _) => Credential::Bearer(token.clone()),
            (None, Some(password), _) | (None, None, Some(password)) => {
                Credential::Password(password.clone())
            }
            // Nothing to log in with: the password is what is missing.
            (None, None, None) => return Move::Fail(SignInFault::Refused),
        };
        let address = label.0.as_str();
        let login = LoginName(typed.login().unwrap_or(address).to_owned());
        let built = match &typed {
            Manual::Imap(servers) => mail_typed(Family::Imap, servers, address, &self.spec)
                .map_err(|_| SignInFault::Unreadable),
            Manual::Pop3(servers) => mail_typed(Family::Pop3, servers, address, &self.spec)
                .map_err(|_| SignInFault::Unreadable),
            Manual::Jmap(server) => {
                jmap::session(&self.io, &server.session, &login, &incoming).await
            }
        };
        let (endpoints, claims) = match built {
            Ok(built) => built,
            Err(fault) => return Move::Fail(fault),
        };
        if let Err(fault) = self.login.check(&incoming, &endpoints).await {
            return Move::Fail(fault);
        }
        if outgoing_changed(current, &typed, password.as_ref())
            && let Some(smtp) = endpoints.iter().find(|e| e.family == Family::Smtp)
            && let Err(fault) = self.login.check_at(&incoming, smtp).await
        {
            return Move::Fail(fault);
        }
        Move::Done(Signed::new(
            label.clone(),
            Vec::new(),
            claims,
            endpoints,
            Restriction::none(),
        ))
    }
}

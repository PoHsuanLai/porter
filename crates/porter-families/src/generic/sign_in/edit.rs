//! An edit of a mail account's servers (`SignInMode::EditServers`): the servers typed are tried
//! before the sheet keeps anything. The incoming server is logged in to with the password typed,
//! or the stored one when none was; the outgoing (SMTP) server too, with `EHLO` and `AUTH`, when
//! it, the login name or the password changed; a JMAP session is fetched over https. A refusal
//! names the field it was about and the sign-in goes on, so the person may fix it and send again.

use super::super::{jmap, mail};
use super::GenericSignIn;
use crate::password::typed_password;
use porter_core::sheet::{
    FieldAnswer, FieldKind, FieldProblem, Manual, ProblemKind, SignInFault, parse_manual,
};
use porter_core::{Credential, Family, LoginName, Restriction, SecretPurpose, SecretText};
use porter_provider::{SignInMode, SignInStep, Signed};

/// The field a fault of the check is about: a refused credential is the password's (or the
/// token's, when one was typed), anything else the server's.
fn refused(fault: SignInFault, credential: FieldKind, server: FieldKind) -> SignInStep {
    let field = match fault {
        SignInFault::Refused => credential,
        _ => server,
    };
    SignInStep::Refused(FieldProblem {
        field,
        problem: ProblemKind::Refused,
    })
}

/// The first stored secret under one of `purposes`, in that order.
fn stored(held: &[(SecretPurpose, Credential)], purposes: &[SecretPurpose]) -> Option<Credential> {
    purposes.iter().find_map(|purpose| {
        held.iter()
            .find(|(p, _)| p == purpose)
            .map(|(_, credential)| credential.clone())
    })
}

/// Whether the outgoing server is to be tried: it, or the login name, differs from what the
/// account has, or a new password was typed.
fn outgoing_changed(current: &Manual, typed: &Manual, password: Option<&SecretText>) -> bool {
    let outgoing = |manual: &Manual| match manual {
        Manual::Imap(servers) | Manual::Pop3(servers) => Some(servers.outgoing.clone()),
        Manual::Jmap(_) => None,
    };
    password.is_some() || outgoing(current) != outgoing(typed) || current.login() != typed.login()
}

impl GenericSignIn {
    /// Tries the servers in `answers` for the account being edited, and ends `Done` with them
    /// only when they let it in.
    pub(super) async fn edit(&self, answers: &[FieldAnswer]) -> SignInStep {
        let SignInMode::EditServers {
            label,
            current,
            credentials,
            ..
        } = &self.mode
        else {
            return SignInStep::Failed(SignInFault::Unreadable);
        };
        let Ok(typed) = parse_manual(answers) else {
            return SignInStep::Failed(SignInFault::Unreadable);
        };
        let password = typed_password(answers);
        let kept = [SecretPurpose::Password, SecretPurpose::IncomingPassword];
        let token = match &typed {
            Manual::Jmap(server) => server.token.clone(),
            Manual::Imap(_) | Manual::Pop3(_) => None,
        };
        let credential_field = match token {
            Some(_) => FieldKind::Token,
            None => FieldKind::Password,
        };
        let incoming = match (&token, &password) {
            (Some(token), _) => Some(Credential::Bearer(token.clone())),
            (None, Some(password)) => Some(Credential::Password(password.clone())),
            (None, None) => stored(credentials, &kept),
        };
        let Some(incoming) = incoming else {
            return SignInStep::Refused(FieldProblem {
                field: FieldKind::Password,
                problem: ProblemKind::Missing,
            });
        };
        let address = label.0.clone();
        let login = LoginName(typed.login().map_or(address.clone(), str::to_owned));
        let built = match &typed {
            Manual::Imap(servers) => mail::typed(Family::Imap, servers, &address, &self.spec),
            Manual::Pop3(servers) => mail::typed(Family::Pop3, servers, &address, &self.spec),
            Manual::Jmap(server) => {
                jmap::session(&self.io, &server.session, &login, &incoming).await
            }
        };
        let (endpoints, claims) = match built {
            Ok(built) => built,
            Err(fault) => {
                let server = match typed {
                    Manual::Jmap(_) => FieldKind::SessionUrl,
                    Manual::Imap(_) | Manual::Pop3(_) => FieldKind::Server,
                };
                return refused(fault, credential_field, server);
            }
        };
        if let Err(fault) = self.login.check(&incoming, &endpoints).await {
            return refused(fault, credential_field, FieldKind::Server);
        }
        if outgoing_changed(current, &typed, password.as_ref())
            && let Some(smtp) = endpoints.iter().find(|e| e.family == Family::Smtp)
        {
            let outgoing = match &password {
                Some(password) => Credential::Password(password.clone()),
                None => stored(credentials, &[SecretPurpose::OutgoingPassword])
                    .unwrap_or_else(|| incoming.clone()),
            };
            if let Err(fault) = self.login.check_at(&outgoing, smtp).await {
                return refused(fault, FieldKind::Password, FieldKind::OutgoingServer);
            }
        }
        let kept: Vec<(SecretPurpose, Credential)> = match (token, password) {
            (Some(token), _) => vec![(SecretPurpose::Password, Credential::Bearer(token))],
            (None, Some(password)) => {
                vec![(SecretPurpose::Password, Credential::Password(password))]
            }
            (None, None) => Vec::new(),
        };
        SignInStep::Done(Signed::new(
            label.clone(),
            kept,
            claims,
            endpoints,
            Restriction::none(),
        ))
    }
}

//! The password tried at the incoming mail server before the account is added: what to try it
//! against. The login itself is the relay's, which dials, upgrades and authenticates.

use porter_capability::CapabilityKind;
use porter_core::{Credential, EndpointProtocol, Reach, RelayAuth, RelayPlan, ServiceEndpoint};

/// The endpoint the account reads mail from.
pub fn incoming(endpoints: &[ServiceEndpoint]) -> Option<&ServiceEndpoint> {
    endpoints.iter().find(|e| {
        matches!(
            e.protocol(),
            Some(EndpointProtocol::Imap | EndpointProtocol::Pop3)
        )
    })
}

/// The relay plan that tries `credential` at the first incoming mail server of `endpoints`.
/// `None` when there is nothing to try (a token, a JMAP-only account): the token's own call
/// already proved it.
pub fn plan(credential: &Credential, endpoints: &[ServiceEndpoint]) -> Option<RelayPlan> {
    plan_at(credential, incoming(endpoints)?)
}

/// The relay plan that tries `credential` at `endpoint`, a mail server (IMAP, POP3 or SMTP: the
/// relay says `EHLO` or `CAPABILITY` and authenticates before its first write). `None` when
/// there is nothing to try (a token).
pub fn plan_at(credential: &Credential, endpoint: &ServiceEndpoint) -> Option<RelayPlan> {
    let Credential::Password(password) = credential else {
        return None;
    };
    Some(RelayPlan::new(
        endpoint.clone(),
        CapabilityKind::Mail,
        RelayAuth::Password(password.clone()),
        Reach::origin(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{EndpointUrl, Family, LoginName, SecretText, Tls};

    fn endpoint(family: Family, url: &str, tls: Tls) -> ServiceEndpoint {
        ServiceEndpoint {
            family,
            url: EndpointUrl::parse(url).unwrap(),
            tls,
            login: LoginName("ada".into()),
        }
    }

    #[test]
    fn a_password_is_tried_at_the_first_incoming_server_and_a_token_is_not_tried() {
        let endpoints = [
            endpoint(Family::Smtp, "smtps://smtp.example.test:465", Tls::Implicit),
            endpoint(Family::Imap, "imaps://imap.example.test:993", Tls::Implicit),
            endpoint(Family::Pop3, "pop3s://pop.example.test:995", Tls::Implicit),
        ];
        let planned = plan(&Credential::Password(SecretText::new("pw")), &endpoints).unwrap();
        assert_eq!(planned.endpoint.family, Family::Imap);
        assert!(plan(&Credential::Bearer(SecretText::new("t")), &endpoints).is_none());
        assert!(
            plan(
                &Credential::Password(SecretText::new("pw")),
                &endpoints[..1]
            )
            .is_none()
        );
    }
}

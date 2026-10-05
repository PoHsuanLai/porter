//! The mail endpoints and claim every mail source builds: autoconfig and SRV name IMAP and
//! SMTP servers, JMAP names its own.

use porter_core::capability::{
    Access, Capability, Delta, LabelModel, MailCap, MailTransport, Offered,
};
use porter_core::{
    Claim, EndpointUrl, Family, LoginName, Offer, Provenance, ServiceEndpoint, Subject, Tls,
};

/// An IMAP endpoint at `host:port`, or `None` when the pieces do not make a coherent one.
pub(crate) fn imap_endpoint(
    host: String,
    port: u16,
    tls: Tls,
    login: LoginName,
) -> Option<ServiceEndpoint> {
    let scheme = if tls == Tls::Implicit {
        "imaps"
    } else {
        "imap"
    };
    endpoint(Family::Imap, scheme, host, port, tls, login)
}

/// An SMTP submission endpoint at `host:port`.
pub(crate) fn smtp_endpoint(
    host: String,
    port: u16,
    tls: Tls,
    login: LoginName,
) -> Option<ServiceEndpoint> {
    let scheme = if tls == Tls::Implicit {
        "smtps"
    } else {
        "smtp"
    };
    endpoint(Family::Smtp, scheme, host, port, tls, login)
}

fn endpoint(
    family: Family,
    scheme: &str,
    host: String,
    port: u16,
    tls: Tls,
    login: LoginName,
) -> Option<ServiceEndpoint> {
    let url = EndpointUrl::parse(&format!("{scheme}://{host}:{port}")).ok()?;
    let endpoint = ServiceEndpoint {
        family,
        url,
        tls,
        login,
    };
    endpoint.check().ok().map(|()| endpoint)
}

/// A mailbox over IMAP, with what a server's naming says and no more: reading and writing, send
/// through the SMTP endpoint, change detection by polling until IMAP's CAPABILITY says IDLE.
pub(crate) fn imap_claim() -> Claim {
    mail_claim(MailTransport::Imap, Offered::Present)
}

/// A mailbox over `transport`, sending as `send` says.
pub(crate) fn mail_claim(transport: MailTransport, send: Offered) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(Capability::Mail(MailCap {
            access: Access::ReadWrite,
            send,
            delta: Delta::Poll,
            transport,
            labels: LabelModel::Folders,
        })),
        provenance: Provenance::Discovered,
    }
}

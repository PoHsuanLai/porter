//! Mozilla-style autoconfig: `https://autoconfig.<domain>/mail/config-v1.1.xml?emailaddress=...`
//! and `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`, then Thunderbird's
//! database.

use crate::found::{DiscoverFault, Found};
use porter_core::EndpointUrl;
use porter_provider::DomainName;

/// The URLs to try for an address at `domain`, in order.
pub fn autoconfig_urls(domain: &DomainName, address: &str) -> Vec<EndpointUrl> {
    let _ = (domain, address);
    todo!("the provider's own autoconfig host, the well-known path, then the ISP database")
}

/// The servers a `config-v1.1.xml` document names for `address`: incoming IMAP (POP3 is
/// mailo's), outgoing SMTP, each with its security and login pattern resolved.
pub fn parse_autoconfig(xml: &str, address: &str) -> Result<Found, DiscoverFault> {
    let _ = (xml, address);
    todo!("an XML reader is needed in quire's pinned block (FINDINGS.md); port mailo's tests")
}

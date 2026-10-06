//! Discovery (porter PLAN §2.7), from mailo's `discover` and `mail-proto::discover`: one
//! function per [`porter_provider::Discovery`] kind. Each is a pure parser of what a server
//! answered, plus the seams it asks through: [`porter_http::Http`] for fetches and [`Dns`] for
//! SRV and MX records. A fake of each is how the tests run without a network. [`discover_mail`]
//! is the mail search over all of them, in mailo's order.
//!
//! `HickoryDns` (feature `io`) is the system resolver; the HTTP client is porter-http's.

mod autoconfig;
mod config;
mod dns;
#[cfg(feature = "io")]
mod dns_io;
mod found;
mod mail;
mod mx;
mod ocs;
#[cfg(test)]
mod options;
mod probe;
mod search;
mod srv;
#[cfg(test)]
mod testing;
mod well_known;

pub use autoconfig::{ISPDB, autoconfig_urls, parse_autoconfig, parse_autoconfig_with};
pub use dns::{Dns, DnsFault, MxRecord, SrvRecord};
#[cfg(feature = "io")]
pub use dns_io::HickoryDns;
pub use found::{
    Direction, DiscoverFault, Found, OAuthOffer, OAuthOnly, OAuthServer, Pop3, Pop3Server,
    SearchOptions, Source, StartTlsOnly,
};
pub use mx::{ProviderLead, ispdb_candidates, lookup_mx, mx_hosts, provider_leads};
pub use ocs::parse_ocs_capabilities;
pub use probe::{ProbeHit, probe_ports};
pub use search::{Miss, NotFound, Outcome, Tried, discover_mail, discover_mail_with};
pub use srv::{SrvAnswers, found_from_srv, lookup_srv, srv_names};
pub use well_known::{discover_well_known, parse_jmap_session, well_known_found, well_known_urls};

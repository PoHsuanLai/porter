//! Discovery (porter PLAN §2.7), from mailo's `discover` and `mail-proto::discover`: one
//! function per [`porter_provider::Discovery`] kind. Each is a pure parser of what a server
//! answered, plus the seams it asks through: [`porter_http::Http`] for fetches and [`Dns`] for
//! SRV and MX records. A fake of each is how the tests run without a network.

mod autoconfig;
mod dns;
mod found;
mod ocs;
mod probe;
mod well_known;

pub use autoconfig::{autoconfig_urls, parse_autoconfig};
pub use dns::{Dns, DnsFault, MxRecord, SrvRecord};
pub use found::{DiscoverFault, Found, Source};
pub use ocs::parse_ocs_capabilities;
pub use probe::{ProbeHit, probe_ports};
pub use well_known::{parse_jmap_session, well_known_urls};

//! Nextcloud's `/ocs/v2.php/cloud/capabilities` answer.

use crate::found::{DiscoverFault, Found};

/// What a Nextcloud's OCS capabilities answer says the account can do: files and quota always;
/// calendar, contacts, tasks and notes when their apps are on the server (an app that is not
/// there is `Absent { NotOnServer }`).
pub fn parse_ocs_capabilities(json: &str) -> Result<Found, DiscoverFault> {
    let _ = json;
    todo!("read `ocs.data.capabilities` and the installed apps; port from the recorded fixtures")
}

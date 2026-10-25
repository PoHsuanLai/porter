//! A typed view of one response's CardDAV properties: what discovery, sync and multiget ask
//! for. Only properties the server found are read; one from a failed `propstat` is absent
//! rather than empty.

use crate::multistatus::Response;
use crate::names::{
    ADDRESS_DATA, ADDRESSBOOK_HOME_SET, CARDDAV, CURRENT_USER_PRINCIPAL, DAV, DISPLAYNAME, GETETAG,
    RESOURCETYPE, SYNC_TOKEN, split,
};

/// The CardDAV properties of one response, as far as the server gave them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Props {
    /// The kinds its `resourcetype` names.
    pub resource: Vec<Resource>,
    /// `current-user-principal`: the principal's href.
    pub current_user_principal: Option<String>,
    /// `addressbook-home-set`: collections whose children are the principal's address books.
    pub addressbook_home_set: Vec<String>,
    /// `displayname`.
    pub display_name: Option<String>,
    /// `getetag`, quotes included: it is compared and sent back in `If-Match`, never read.
    pub etag: Option<String>,
    /// `sync-token` as a property of the collection.
    pub sync_token: Option<String>,
    /// `address-data`: the card itself, as vCard text.
    pub address_data: Option<String>,
}

/// One kind named in a `resourcetype`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    /// `DAV:collection`.
    Collection,
    /// A CardDAV address book (RFC 6352 section 5.2).
    AddressBook,
    /// `DAV:principal`.
    Principal,
    /// Anything else, by its local name: a calendar, a server's own extension.
    Other(String),
}

impl Props {
    /// The properties `response` carries with a 2xx status.
    pub fn of(response: &Response) -> Self {
        let found = |name: &str| response.found(name).map(str::to_owned);
        let hrefs = |name: &str| -> Vec<String> {
            // The value lists an href's text and any other child by its qualified name; a
            // principal or home set holds only hrefs, or `DAV:unauthenticated`, which is none.
            response
                .found(name)
                .map(|v| {
                    v.split_whitespace()
                        .filter(|word| !word.starts_with(DAV))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Props {
            resource: response
                .found(RESOURCETYPE)
                .map(|v| v.split_whitespace().map(resource).collect())
                .unwrap_or_default(),
            current_user_principal: hrefs(CURRENT_USER_PRINCIPAL).into_iter().next(),
            addressbook_home_set: hrefs(ADDRESSBOOK_HOME_SET),
            display_name: found(DISPLAYNAME),
            etag: found(GETETAG),
            sync_token: found(SYNC_TOKEN),
            address_data: found(ADDRESS_DATA),
        }
    }

    /// Whether the resource is an address book.
    pub fn is_address_book(&self) -> bool {
        self.resource.contains(&Resource::AddressBook)
    }
}

impl From<&Response> for Props {
    fn from(response: &Response) -> Self {
        Props::of(response)
    }
}

fn resource(qualified: &str) -> Resource {
    match split(qualified) {
        (DAV, "collection") => Resource::Collection,
        (DAV, "principal") => Resource::Principal,
        (CARDDAV, "addressbook") => Resource::AddressBook,
        (_, local) => Resource::Other(local.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_multistatus;

    #[test]
    fn an_unauthenticated_principal_is_no_principal() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/</d:href>
          <d:propstat><d:prop><d:current-user-principal><d:unauthenticated/></d:current-user-principal></d:prop>
          <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        let props = Props::of(&parse_multistatus(xml).unwrap().responses[0]);
        assert_eq!(props.current_user_principal, None);
    }

    #[test]
    fn a_collection_sync_token_and_a_name_are_read() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/book/</d:href>
          <d:propstat><d:prop><d:displayname>Friends</d:displayname><d:sync-token>s-1</d:sync-token>
          <d:resourcetype><d:collection/><d:principal/></d:resourcetype></d:prop>
          <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        let props = Props::from(&parse_multistatus(xml).unwrap().responses[0]);
        assert_eq!(props.display_name.as_deref(), Some("Friends"));
        assert_eq!(props.sync_token.as_deref(), Some("s-1"));
        assert_eq!(props.resource, [Resource::Collection, Resource::Principal]);
        assert!(!props.is_address_book());
    }
}

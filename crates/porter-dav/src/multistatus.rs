//! A `207 Multi-Status` body.

use thiserror::Error;

/// One property and the status it came with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    /// The property's name, namespace-qualified (`DAV:getetag`).
    pub name: String,
    /// Its text, empty for a property with no value.
    pub value: String,
    /// The status of the property's group.
    pub status: PropStatus,
}

/// The status of a property group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropStatus {
    /// 2xx.
    Found,
    /// 404: the server does not have it.
    Missing,
    /// Another status.
    Other(u16),
}

/// One `response` element: an href and its properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The resource, as written (a path or a URL).
    pub href: String,
    /// Its properties.
    pub props: Vec<Prop>,
}

/// A parsed multistatus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Multistatus {
    /// The responses, in order.
    pub responses: Vec<Response>,
}

/// Why a body was not a multistatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DavFault {
    /// Not XML, or not a `multistatus`.
    #[error("not a multistatus")]
    Unreadable,
}

/// Reads a multistatus body.
pub fn parse_multistatus(xml: &str) -> Result<Multistatus, DavFault> {
    let _ = xml;
    todo!(
        "an XML reader is needed in quire's pinned block (FINDINGS.md); port mail-pim's tests and add recorded Nextcloud and Fastmail fixtures"
    )
}

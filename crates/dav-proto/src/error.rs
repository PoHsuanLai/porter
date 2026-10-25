//! Why a reply could not be read.

use thiserror::Error as ThisError;

/// Why a body was not a multistatus.
#[derive(Debug, Clone, PartialEq, Eq, ThisError)]
#[non_exhaustive]
pub enum Error {
    /// Not well-formed XML, or XML with a document type declaration, which is refused rather
    /// than expanded. The text is the XML reader's own.
    #[error("not XML: {0}")]
    Xml(String),
    /// XML whose root is not `DAV:multistatus`: a sign-in page, or another error body.
    #[error("not a multistatus")]
    NotMultistatus,
}

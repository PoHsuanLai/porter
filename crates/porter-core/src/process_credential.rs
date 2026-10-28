//! The id of a credential handed to a spawned process (`Tokens.IssueProcessCredential`, agent
//! session ask P2). accountd makes it; the launcher names it to end the credential and is told it
//! when accountd ends one. It is not a secret and it is never derived from one.

use porter_ids::IdError;
use porter_ids::is_id;
use serde::{Deserialize, Serialize};

/// One handed-off credential. The same grammar as every id of ours, so it is also safe as a
/// single path segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProcessCredentialId(Box<str>);

impl ProcessCredentialId {
    /// The id written as `text`.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        match is_id(text) {
            true => Ok(Self(text.into())),
            false => Err(IdError::malformed("process credential id", text)),
        }
    }
}

porter_ids::string_newtype!(ProcessCredentialId, IdError);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_the_id_grammar_and_so_one_safe_path_segment() {
        for good in ["cred-1", "cred-12-ab34", "a.b_c"] {
            let id = ProcessCredentialId::parse(good).expect(good);
            assert_eq!(id.as_str(), good);
            assert!(!id.as_str().contains('/'));
        }
        for bad in ["", "../x", "a/b", "Cred", "-x", "a b", ".."] {
            assert!(ProcessCredentialId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn it_reads_and_writes_as_its_text() {
        let id = ProcessCredentialId::parse("cred-7").expect("id");
        assert_eq!(serde_json::to_string(&id).expect("json"), r#""cred-7""#);
        assert_eq!(
            serde_json::from_str::<ProcessCredentialId>(r#""cred-7""#).expect("id"),
            id
        );
        assert!(serde_json::from_str::<ProcessCredentialId>(r#""a/b""#).is_err());
    }
}

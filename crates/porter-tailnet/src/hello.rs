//! The hello: what a lending computer answers to `GET /hello` from one of the person's own
//! computers that wants to know whether it lends. It names the lending service and the models it
//! offers, and whether the asking computer still has to be allowed; it says nothing else about
//! the computer.

use serde::{Deserialize, Serialize};

/// The path a hello is asked for.
pub const HELLO_PATH: &str = "/hello";

/// What `service` says in every hello of this program.
const SERVICE: &str = "inferd";

/// The version of the hello's form.
const VERSION: u32 = 1;

/// The most models a hello holds (what it reads back).
const MOST_MODELS: usize = 256;

/// One model a computer lends: its id in the catalogue both computers share, and the name shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LentModel {
    /// The catalogue id.
    pub id: String,
    /// The name a person reads.
    pub name: String,
}

/// A hello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    service: String,
    version: u32,
    models: Vec<LentModel>,
    needs_approval: bool,
}

/// Where the asking computer stands with the person on the lending one. It lives in
/// `porter_core::lending` (a client of the bus reads it without this crate's files); the old
/// path stays.
pub use porter_core::lending::Approval;

impl Hello {
    /// A hello offering `models` to a computer with this `approval`.
    pub fn new(models: Vec<LentModel>, approval: Approval) -> Self {
        Self {
            service: SERVICE.to_owned(),
            version: VERSION,
            models,
            needs_approval: approval == Approval::Needed,
        }
    }

    /// The models offered.
    pub fn models(&self) -> &[LentModel] {
        &self.models
    }

    /// Whether the asking computer must still be allowed on the lending one.
    pub fn needs_approval(&self) -> bool {
        self.needs_approval
    }

    /// The hello as the JSON it is answered in.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a hello; none when the text is not one of this program's (another service on the
    /// port, a newer form, too many models).
    pub fn parse(body: &[u8]) -> Option<Self> {
        let hello: Hello = serde_json::from_slice(body).ok()?;
        let fine = hello.service == SERVICE
            && hello.version == VERSION
            && hello.models.len() <= MOST_MODELS;
        fine.then_some(hello)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> LentModel {
        LentModel {
            id: id.to_owned(),
            name: id.to_uppercase(),
        }
    }

    #[test]
    fn a_hello_reads_back_as_it_was_written() {
        let hello = Hello::new(vec![model("qwen"), model("gemma")], Approval::Needed);
        assert_eq!(Hello::parse(hello.to_json().as_bytes()), Some(hello));
    }

    #[test]
    fn a_hello_says_the_service_the_models_and_the_approval_and_nothing_else() {
        let value: serde_json::Value =
            serde_json::from_str(&Hello::new(vec![model("qwen")], Approval::Given).to_json())
                .unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["models", "needs_approval", "service", "version"]);
    }

    #[test]
    fn what_is_not_this_programs_hello_is_not_read() {
        let other_service = r#"{"service":"web","version":1,"models":[],"needs_approval":false}"#;
        let newer = r#"{"service":"inferd","version":2,"models":[],"needs_approval":false}"#;
        for body in [other_service, newer, "{}", "[]", "not json", ""] {
            assert_eq!(Hello::parse(body.as_bytes()), None, "{body}");
        }
        let many = Hello::new(
            (0..=MOST_MODELS).map(|n| model(&format!("m{n}"))).collect(),
            Approval::Given,
        );
        assert_eq!(Hello::parse(many.to_json().as_bytes()), None);
    }
}

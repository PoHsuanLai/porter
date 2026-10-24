//! An online document's link file: what reading `<name>.gdoc` gives, made here and never cached.
//! A JSON object whose `url` is where the document opens and whose `doc_id` is the drive's id
//! for it, as Google's own desktop client writes them.

use porter_sync::RemoteId;

/// The link file of the document `id` that opens at `url`.
pub fn link_file(url: &str, id: &RemoteId) -> Vec<u8> {
    let quote = |text: &str| serde_json::Value::String(text.to_owned()).to_string();
    format!(
        "{{\"url\": {}, \"doc_id\": {}}}\n",
        quote(url),
        quote(&id.0)
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_file_is_json_with_the_address_and_the_id() {
        let bytes = link_file(
            "https://docs.example.test/d/1?a=\"b\"",
            &RemoteId("1".into()),
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["url"], "https://docs.example.test/d/1?a=\"b\"");
        assert_eq!(value["doc_id"], "1");
    }
}

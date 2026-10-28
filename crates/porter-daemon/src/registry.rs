//! What a daemon tells the person when its registry file is refused at start. The three daemons
//! (accountd, syncd, inferd) refuse the same way and differ only in the service they name, so
//! the paragraph is written once, here, from the parts of the library's refusal.

use std::path::Path;

/// The words for a registry file refused at start: the file by name, why, that nothing was
/// changed and that `service` (as the person knows it: "accounts service") did not start, and
/// the copy from before the last change when there is one.
pub fn registry_words(file: &Path, backup: Option<&Path>, cause: &str, service: &str) -> String {
    let spare = match backup {
        Some(backup) => format!(
            "The copy from before the last change is {}. To go back to it, stop the {service}, \
             put that copy in place of the first file, and start it again.",
            backup.display()
        ),
        None => "There is no earlier copy.".to_string(),
    };
    format!(
        "the accounts file {} cannot be read ({cause}). Nothing was changed and the {service} \
         did not start, so no account is lost. {spare}",
        file.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_name_the_files_the_service_and_that_nothing_changed() {
        let said = registry_words(
            Path::new("/r/registry.json"),
            Some(Path::new("/r/registry.json.bak")),
            "oops",
            "sync service",
        );
        for part in [
            "/r/registry.json ",
            "/r/registry.json.bak",
            "oops",
            "sync service",
            "Nothing was changed",
        ] {
            assert!(said.contains(part), "{said}");
        }
        let alone = registry_words(Path::new("/r/registry.json"), None, "oops", "sync service");
        assert!(alone.contains("no earlier copy"), "{alone}");
    }
}

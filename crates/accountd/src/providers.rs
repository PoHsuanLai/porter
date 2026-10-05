//! The provider files the daemon serves, and the family that serves each. Files are read from
//! the directories in order, a later directory's file replacing an earlier one's of the same
//! id (the user's over the system's); a file that does not parse is skipped and named on
//! standard error, never fatal. A provider whose sign-in no built family serves (Google, which
//! is a TODO; a local runtime, which inferd reports) is kept out of the served set.

use porter_core::AuthKind;
use porter_families::{
    ApiKeyProvider, FamilyProvider, GenericProvider, MicrosoftProvider, NextcloudProvider,
    OpenRouterProvider,
};
use porter_provider::{Issuer, ProviderSpec, parse_provider};
use std::path::{Path, PathBuf};

/// What reading the directories found.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The providers, one per id.
    pub specs: Vec<ProviderSpec>,
    /// Files skipped, each with why.
    pub skipped: Vec<(PathBuf, String)>,
}

/// The provider files of `dirs`, later directories winning; a missing directory is empty.
pub fn load_specs(dirs: &[PathBuf]) -> Loaded {
    let mut out = Loaded::default();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for file in files {
            match read(&file) {
                Ok(spec) => {
                    out.specs.retain(|s| s.id != spec.id);
                    out.specs.push(spec);
                }
                Err(why) => out.skipped.push((file, why)),
            }
        }
    }
    out
}

fn read(file: &Path) -> Result<ProviderSpec, String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let spec = parse_provider(&text).map_err(|e| e.to_string())?;
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match spec.id.as_str() == stem {
        true => Ok(spec),
        false => Err(format!("the id `{}` is not the file name", spec.id)),
    }
}

/// The family that serves `spec`, or the spec back when none is built for it.
pub fn family_of(spec: ProviderSpec) -> Result<FamilyProvider, Box<ProviderSpec>> {
    match (spec.auth.kind, spec.auth.issuer) {
        (AuthKind::LoginFlowV2, _) => Ok(FamilyProvider::Nextcloud(NextcloudProvider::new(spec))),
        (AuthKind::Password | AuthKind::AppPassword, _) => {
            Ok(FamilyProvider::Generic(GenericProvider::new(spec)))
        }
        (AuthKind::OAuthPkce, Some(Issuer::Microsoft)) => {
            Ok(FamilyProvider::Microsoft(MicrosoftProvider::new(spec)))
        }
        (AuthKind::ApiKey, _) => Ok(FamilyProvider::ApiKey(ApiKeyProvider::new(spec))),
        (AuthKind::OAuthMintsKey, Some(Issuer::OpenRouter)) => {
            Ok(FamilyProvider::OpenRouter(OpenRouterProvider::new(spec)))
        }
        _ => Err(Box::new(spec)),
    }
}

/// The families that serve `specs`, and the providers none serves.
pub fn served(specs: Vec<ProviderSpec>) -> (Vec<FamilyProvider>, Vec<ProviderSpec>) {
    let (mut families, mut unserved) = (Vec::new(), Vec::new());
    for spec in specs {
        match family_of(spec) {
            Ok(family) => families.push(family),
            Err(spec) => unserved.push(*spec),
        }
    }
    (families, unserved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("accountd-providers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn shipped() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers")
    }

    #[test]
    fn the_shipped_files_load_and_each_is_served_or_named_unserved() {
        let loaded = load_specs(&[shipped()]);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        let (families, unserved) = served(loaded.specs);
        let ids: Vec<String> = unserved.iter().map(|s| s.id.to_string()).collect();
        assert!(!families.is_empty());
        // Google has no family (a TODO); the local runtimes are reported by inferd.
        assert!(ids.contains(&"google".to_owned()), "{ids:?}");
        assert!(!ids.contains(&"nextcloud".to_owned()), "{ids:?}");
    }

    #[test]
    fn a_later_directory_replaces_a_file_of_the_same_id_and_bad_files_are_skipped() {
        let (system, user) = (scratch("system"), scratch("user"));
        let nextcloud = std::fs::read_to_string(shipped().join("nextcloud.toml")).expect("file");
        std::fs::write(system.join("nextcloud.toml"), &nextcloud).expect("write");
        std::fs::write(
            user.join("nextcloud.toml"),
            nextcloud.replace("label = \"Nextcloud\"", "label = \"Mine\""),
        )
        .expect("write");
        std::fs::write(user.join("broken.toml"), "id = ").expect("write");
        std::fs::write(user.join("mismatch.toml"), nextcloud.clone()).expect("write");
        let loaded = load_specs(&[system.clone(), user.clone(), PathBuf::from("/nonexistent")]);
        assert_eq!(loaded.specs.len(), 1);
        assert_eq!(loaded.specs[0].label, "Mine");
        assert_eq!(loaded.skipped.len(), 2);
        let _ = std::fs::remove_dir_all(system);
        let _ = std::fs::remove_dir_all(user);
    }
}

//! The provider files the daemon serves, and the family that serves each. Files are read from
//! the directories in order, a later directory's file replacing an earlier one's of the same
//! id (the user's over the system's); a file that does not parse is skipped and named on
//! standard error, never fatal. A provider whose sign-in no built family serves (Google, which
//! is a TODO; a local runtime, which inferd reports) is kept out of the served set.

use porter_core::AuthKind;
use porter_discover::{Dns, DnsFault, HickoryDns, MxRecord, SrvRecord};
use porter_families::{
    ApiKeyProvider, FamilyProvider, GenericProvider, MicrosoftProvider, NextcloudProvider,
    OpenRouterProvider, SharedDns,
};
use porter_http::{HyperHttp, SharedHttp, TokioSleep};
use porter_provider::{DomainName, Issuer, ProviderSet, ProviderSpec, parse_provider};
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

/// What the families dial and resolve through: one HTTP client and one resolver for all of
/// them, and the providers that may claim an address before the generic family searches.
#[derive(Debug, Clone)]
pub struct FamilyIo {
    /// The HTTP client.
    pub http: SharedHttp,
    /// The resolver.
    pub dns: SharedDns,
    /// Every loaded provider.
    pub providers: ProviderSet,
}

impl FamilyIo {
    /// The system's: hyper over the platform's roots and the system resolver, or a resolver that
    /// answers `Unreachable` when the system names none (discovery then falls back to
    /// autoconfig and a server the person types).
    pub fn system(providers: ProviderSet) -> Self {
        let dns = match HickoryDns::system() {
            Ok(dns) => SharedDns::new(dns),
            Err(_) => SharedDns::new(NoResolver),
        };
        Self {
            http: SharedHttp::new(HyperHttp::new()),
            dns,
            providers,
        }
    }
}

/// The resolver when the system names none: every lookup is `Unreachable`.
#[derive(Debug, Clone, Copy)]
pub struct NoResolver;

impl Dns for NoResolver {
    async fn srv(&self, _name: &str) -> Result<Vec<SrvRecord>, DnsFault> {
        Err(DnsFault::Unreachable)
    }

    async fn mx(&self, _domain: &DomainName) -> Result<Vec<MxRecord>, DnsFault> {
        Err(DnsFault::Unreachable)
    }
}

/// The family that serves `spec` through `io`, or the spec back when none is built for it.
pub fn family_of(spec: ProviderSpec, io: &FamilyIo) -> Result<FamilyProvider, Box<ProviderSpec>> {
    match (spec.auth.kind, spec.auth.issuer) {
        (AuthKind::LoginFlowV2, _) => Ok(FamilyProvider::Nextcloud(NextcloudProvider::new(
            spec,
            io.http.clone(),
            TokioSleep,
        ))),
        (AuthKind::Password | AuthKind::AppPassword, _) => Ok(FamilyProvider::Generic(
            GenericProvider::new(spec, io.http.clone(), io.dns.clone())
                .with_providers(io.providers.clone()),
        )),
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

/// The families that serve `specs` through `io`, and the providers none serves.
pub fn served(specs: Vec<ProviderSpec>, io: &FamilyIo) -> (Vec<FamilyProvider>, Vec<ProviderSpec>) {
    let (mut families, mut unserved) = (Vec::new(), Vec::new());
    for spec in specs {
        match family_of(spec, io) {
            Ok(family) => families.push(family),
            Err(spec) => unserved.push(*spec),
        }
    }
    (families, unserved)
}

/// The local runtimes among the providers no family serves (`ollama`, `llama-cpp`, `lm-studio`):
/// they sign in through nothing, and inferd reports them with `Peer.ReportLocal`, so the
/// service keeps them in its catalogue.
pub fn local_runtimes(unserved: &[ProviderSpec]) -> Vec<ProviderSpec> {
    unserved
        .iter()
        .filter(|spec| spec.auth.kind == AuthKind::LocalRuntime)
        .cloned()
        .collect()
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
        let io = FamilyIo {
            http: SharedHttp::new(HyperHttp::new()),
            dns: SharedDns::new(NoResolver),
            providers: ProviderSet::layered(loaded.specs.clone(), Vec::new()),
        };
        let (families, unserved) = served(loaded.specs, &io);
        let ids: Vec<String> = unserved.iter().map(|s| s.id.to_string()).collect();
        assert!(!families.is_empty());
        // Google has no family (a TODO); the local runtimes are reported by inferd.
        assert!(ids.contains(&"google".to_owned()), "{ids:?}");
        assert!(!ids.contains(&"nextcloud".to_owned()), "{ids:?}");
        let local: Vec<String> = local_runtimes(&unserved)
            .iter()
            .map(|s| s.id.to_string())
            .collect();
        for runtime in ["ollama", "llama-cpp", "lm-studio"] {
            assert!(local.contains(&runtime.to_owned()), "{local:?}");
        }
        assert!(!local.contains(&"google".to_owned()), "{local:?}");
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

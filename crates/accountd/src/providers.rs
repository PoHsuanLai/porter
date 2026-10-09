//! The provider files the daemon serves, and the family that serves each. Files are read from
//! the directories in order, a later directory's file replacing an earlier one's of the same
//! id (the user's over the system's); a file that does not parse is skipped and named on
//! standard error, never fatal. The person's own file may not change where a shipped provider
//! signs in or which servers it reaches ([`Layer::Person`]). A provider whose sign-in no built
//! family serves (a local runtime, which inferd reports) is kept out of the served set. Google is
//! served with or without a client id registered: without one, adding it says it needs one.

use porter_core::clock::SystemClock;
use porter_core::{AuthKind, EndpointUrl};
use porter_discover::{Dns, DnsFault, HickoryDns, MxRecord, SrvRecord};
use porter_families::{
    AgentLoginProvider, ApiKeyProvider, ClientFiles, FamilyProvider, GenericProvider, GoogleEnv,
    GoogleProvider, MicrosoftEnv, MicrosoftProvider, NextcloudProvider, SharedDns, TailnetProvider,
};
use porter_http::{HyperHttp, SharedHttp, SharedSleep, TokioSleep};
use porter_provider::{
    DomainName, Issuer, ProviderFileError, ProviderSet, ProviderSpec, parse_provider,
};
use porter_tailscale::LocalApi;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What reading the directories found.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The providers, one per id.
    pub specs: Vec<ProviderSpec>,
    /// Files skipped, each with why.
    pub skipped: Vec<(PathBuf, SkipReason)>,
}

/// Why a provider file was skipped; each keeps what failed, and its text is what is logged.
#[derive(Debug, thiserror::Error)]
pub enum SkipReason {
    /// The file could not be read.
    #[error(transparent)]
    Unreadable(#[from] std::io::Error),
    /// The file is not a provider file.
    #[error(transparent)]
    Malformed(#[from] ProviderFileError),
    /// The file's name is not its provider's id.
    #[error("the id `{0}` is not the file name")]
    NameIsNotId(String),
    /// The person's file would change what a shipped provider trusts.
    #[error("kept the shipped `{id}`: this file would change {what}")]
    WouldWiden {
        /// The provider.
        id: String,
        /// What it would change, in words.
        what: &'static str,
    },
}

/// Whose a provider directory is, which decides what its files may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The system's (`/usr/share/porter/providers`): what this install ships.
    Shipped,
    /// The person's own (`$XDG_DATA_HOME/porter/providers`), which any program of theirs can
    /// write: it may add providers and reword a shipped one, but a file for a shipped provider
    /// that changes its sign-in, or reaches a server, an authenticated origin or a linked origin
    /// the shipped file does not, is skipped and the shipped one kept.
    Person,
    /// A directory named on the command line (`--providers`, a test rig's fakes): as trusted as
    /// whoever started the daemon.
    Named,
}

/// The provider files of `dirs`, later directories winning; a missing directory is empty.
pub fn load_specs(dirs: &[(Layer, PathBuf)]) -> Loaded {
    let mut out = Loaded::default();
    for (layer, dir) in dirs {
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
            let checked = read(&file).and_then(|spec| match layer {
                Layer::Person => keeps_shipped_trust(&out.specs, spec),
                Layer::Shipped | Layer::Named => Ok(spec),
            });
            match checked {
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

/// `spec` when it changes nothing a shipped provider of its id trusts: the one loaded from the
/// system's directory, else the one compiled in. A provider no one ships is the person's own.
fn keeps_shipped_trust(
    loaded: &[ProviderSpec],
    spec: ProviderSpec,
) -> Result<ProviderSpec, SkipReason> {
    let shipped = loaded
        .iter()
        .find(|s| s.id == spec.id)
        .cloned()
        .or_else(|| {
            porter_provider::shipped_specs()
                .into_iter()
                .find(|s| s.id == spec.id)
        });
    match shipped.and_then(|shipped| widened(&shipped, &spec)) {
        Some(what) => Err(SkipReason::WouldWiden {
            id: spec.id.to_string(),
            what,
        }),
        None => Ok(spec),
    }
}

/// What `file` changes of what `shipped` trusts, if anything: its sign-in, or a server, an
/// authenticated origin or a linked origin it does not name. Naming fewer is allowed.
fn widened(shipped: &ProviderSpec, file: &ProviderSpec) -> Option<&'static str> {
    let hosts = |spec: &ProviderSpec| -> BTreeSet<String> {
        spec.capabilities
            .iter()
            .filter_map(|row| row.endpoint.as_ref())
            .map(|e| EndpointUrl::parse(&e.0).map_or_else(|_| e.0.clone(), |u| u.origin().host))
            .collect()
    };
    let origins = |spec: &ProviderSpec, linked: bool| -> BTreeSet<String> {
        spec.capabilities
            .iter()
            .flat_map(|row| match linked {
                true => row
                    .linked_origins
                    .iter()
                    .map(|o| o.to_string())
                    .collect::<Vec<_>>(),
                false => row.auth_origins.iter().map(|o| o.to_string()).collect(),
            })
            .collect()
    };
    if file.auth != shipped.auth {
        return Some("how it signs in");
    }
    if !hosts(file).is_subset(&hosts(shipped)) {
        return Some("the servers it reaches");
    }
    if !origins(file, false).is_subset(&origins(shipped, false)) {
        return Some("the servers that get its sign-in");
    }
    if !origins(file, true).is_subset(&origins(shipped, true)) {
        return Some("the servers its links may reach");
    }
    None
}

fn read(file: &Path) -> Result<ProviderSpec, SkipReason> {
    let text = std::fs::read_to_string(file)?;
    let spec = parse_provider(&text)?;
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match spec.id.as_str() == stem {
        true => Ok(spec),
        false => Err(SkipReason::NameIsNotId(spec.id.to_string())),
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
    /// The clients files the OAuth families read at every sign-in and token: the person's own
    /// is the one Settings writes, so a client id set there is used without a restart.
    pub clients: ClientFiles,
    /// Tailscale on this computer: the socket its program serves.
    pub tailscale: LocalApi,
}

impl FamilyIo {
    /// The system's: hyper over the platform's roots and the system resolver, or a resolver that
    /// answers `Unreachable` when the system names none (discovery then falls back to
    /// autoconfig and a server the person types).
    pub fn system(providers: ProviderSet, clients: ClientFiles) -> Self {
        let dns = match HickoryDns::system() {
            Ok(dns) => SharedDns::new(dns),
            Err(_) => SharedDns::new(NoResolver),
        };
        Self {
            http: SharedHttp::new(HyperHttp::new()),
            dns,
            providers,
            clients,
            tailscale: LocalApi::system(),
        }
    }

    /// The same, asking Tailscale through `tailscale` (a test names its own socket).
    pub fn with_tailscale(self, tailscale: LocalApi) -> Self {
        Self { tailscale, ..self }
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
            Ok(FamilyProvider::Microsoft(MicrosoftProvider::with_env(
                spec,
                MicrosoftEnv::with_client_files(io.clients.clone(), SystemClock),
            )))
        }
        (AuthKind::OAuthPkce, Some(Issuer::Google)) => {
            Ok(FamilyProvider::Google(GoogleProvider::with_env(
                spec,
                GoogleEnv::with_client_files(io.clients.clone(), SystemClock),
            )))
        }
        (AuthKind::ApiKey, _) => Ok(FamilyProvider::ApiKey(ApiKeyProvider::new(spec))),
        (AuthKind::AgentLogin, _) => Ok(FamilyProvider::AgentLogin(AgentLoginProvider::new(spec))),
        (AuthKind::OwnProgram, _) => Ok(FamilyProvider::Tailnet(TailnetProvider::new(
            spec,
            io.tailscale.clone(),
            SharedSleep::new(TokioSleep),
        ))),
        // `OAuthMintsKey` (OpenRouter's browser sign-in) has a skeleton family but no body, so
        // it is not mapped: a file asking for it lands in `unserved` with its logged reason.
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
    use porter_provider::Provider;

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
        let loaded = load_specs(&[(Layer::Shipped, shipped())]);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        let io = FamilyIo {
            http: SharedHttp::new(HyperHttp::new()),
            dns: SharedDns::new(NoResolver),
            providers: ProviderSet::layered(loaded.specs.clone(), Vec::new()),
            clients: ClientFiles {
                shipped: PathBuf::from("/nonexistent/clients.toml"),
                own: PathBuf::from("/nonexistent/own-clients.toml"),
            },
            tailscale: LocalApi::new("/nonexistent/tailscaled.sock"),
        };
        let (families, unserved) = served(loaded.specs, &io);
        let ids: Vec<String> = unserved.iter().map(|s| s.id.to_string()).collect();
        assert!(!families.is_empty());
        // Google has its family; the local runtimes are reported by inferd.
        assert!(!ids.contains(&"google".to_owned()), "{ids:?}");
        assert!(
            families.iter().any(|f| f.spec().id.as_str() == "google"),
            "google is served"
        );
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
        let loaded = load_specs(&[
            (Layer::Shipped, system.clone()),
            (Layer::Person, user.clone()),
            (Layer::Named, PathBuf::from("/nonexistent")),
        ]);
        assert_eq!(loaded.specs.len(), 1);
        assert_eq!(loaded.specs[0].label, "Mine");
        assert_eq!(loaded.skipped.len(), 2);
        // Each reason keeps what failed, not its text.
        let reasons: Vec<&SkipReason> = loaded.skipped.iter().map(|(_, why)| why).collect();
        assert!(
            reasons
                .iter()
                .any(|why| matches!(why, SkipReason::Malformed(_))),
            "{reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|why| matches!(why, SkipReason::NameIsNotId(id) if id == "nextcloud")),
            "{reasons:?}"
        );
        let _ = std::fs::remove_dir_all(system);
        let _ = std::fs::remove_dir_all(user);
    }

    /// A provider file in the person's own folder that asks for OpenRouter's browser sign-in
    /// (`oauth_mints_key`), a family that has no body, under a new id.
    #[test]
    fn a_persons_file_asking_for_an_unbuilt_family_is_not_served_and_names_why() {
        let own = scratch("unbuilt");
        let text = std::fs::read_to_string(shipped().join("openrouter.toml"))
            .expect("file")
            .replace("id = \"openrouter\"", "id = \"my-router\"")
            .replace(
                "kind = \"api_key\"",
                "kind = \"oauth_mints_key\"\nissuer = \"openrouter\"",
            );
        assert!(text.contains("oauth_mints_key"), "{text}");
        std::fs::write(own.join("my-router.toml"), text).expect("write");
        let loaded = load_specs(&[(Layer::Person, own.clone())]);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        assert_eq!(loaded.specs.len(), 1);
        let io = FamilyIo {
            http: SharedHttp::new(HyperHttp::new()),
            dns: SharedDns::new(NoResolver),
            providers: ProviderSet::layered(loaded.specs.clone(), Vec::new()),
            clients: ClientFiles {
                shipped: PathBuf::from("/nonexistent/clients.toml"),
                own: PathBuf::from("/nonexistent/own-clients.toml"),
            },
            tailscale: LocalApi::new("/nonexistent/tailscaled.sock"),
        };
        let (families, unserved) = served(loaded.specs, &io);
        assert!(families.is_empty(), "not served");
        let ids: Vec<String> = unserved.iter().map(|s| s.id.to_string()).collect();
        assert_eq!(ids, ["my-router"]);
        assert!(
            local_runtimes(&unserved).is_empty(),
            "and not a local runtime"
        );
        let _ = std::fs::remove_dir_all(own);
    }

    fn microsoft() -> String {
        std::fs::read_to_string(shipped().join("microsoft.toml")).expect("file")
    }

    /// Loads the shipped `microsoft.toml` from a system directory (or none), then `mine` as the
    /// same id from a directory of `layer`.
    fn over_microsoft(name: &str, system: bool, layer: Layer, mine: &str) -> Loaded {
        let (sys, over) = (
            scratch(&format!("{name}-sys")),
            scratch(&format!("{name}-over")),
        );
        if system {
            std::fs::write(sys.join("microsoft.toml"), microsoft()).expect("write");
        }
        std::fs::write(over.join("microsoft.toml"), mine).expect("write");
        let loaded = load_specs(&[(Layer::Shipped, sys.clone()), (layer, over.clone())]);
        let _ = std::fs::remove_dir_all(sys);
        let _ = std::fs::remove_dir_all(over);
        loaded
    }

    fn microsoft_of(loaded: &Loaded) -> &ProviderSpec {
        loaded
            .specs
            .iter()
            .find(|s| s.id.as_str() == "microsoft")
            .expect("microsoft")
    }

    #[test]
    fn the_persons_file_may_not_move_a_shipped_providers_servers_sign_in_or_origins() {
        let shipped_file = microsoft();
        let cases = [
            (
                "servers",
                shipped_file.replace("https://graph.microsoft.com", "https://graph.evil.test"),
                "the servers it reaches",
            ),
            (
                "sign-in",
                shipped_file.replace("issuer = \"microsoft\"", "issuer = \"google\""),
                "how it signs in",
            ),
            (
                "linked",
                shipped_file.replace("\"*.sharepoint.com\"", "\"*.sharepoint.evil.test\""),
                "the servers its links may reach",
            ),
        ];
        for (name, mine, what) in cases {
            assert_ne!(mine, shipped_file, "{name}: the line was found");
            // Over the system's file (kept), and with none installed (the compiled-in file is
            // the shipped one; nothing serves the id).
            for system in [true, false] {
                let loaded = over_microsoft(name, system, Layer::Person, &mine);
                match system {
                    true => assert_eq!(
                        microsoft_of(&loaded),
                        &porter_provider::shipped_specs()
                            .into_iter()
                            .find(|s| s.id.as_str() == "microsoft")
                            .expect("shipped"),
                        "{name}"
                    ),
                    false => assert!(loaded.specs.is_empty(), "{name}"),
                }
                let [(_, why)] = loaded.skipped.as_slice() else {
                    panic!("{name} {system}: one skipped: {:?}", loaded.skipped);
                };
                assert!(
                    matches!(why, SkipReason::WouldWiden { what: got, .. } if *got == what),
                    "{name}: {why}"
                );
            }
        }
    }

    #[test]
    fn the_persons_file_may_reword_a_shipped_provider_and_a_named_directory_may_move_it() {
        let reworded = microsoft().replace("label = \"Microsoft\"", "label = \"Work\"");
        assert_ne!(reworded, microsoft());
        let loaded = over_microsoft("reword", true, Layer::Person, &reworded);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        assert_eq!(microsoft_of(&loaded).label, "Work");

        let moved = microsoft().replace("https://graph.microsoft.com", "https://127.0.0.1:4443");
        let loaded = over_microsoft("named", true, Layer::Named, &moved);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        let graph = microsoft_of(&loaded)
            .capabilities
            .iter()
            .filter_map(|row| row.endpoint.as_ref())
            .any(|e| e.0 == "https://127.0.0.1:4443");
        assert!(graph);
    }
}

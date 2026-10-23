//! A scripted stand-in for the AI accounts inferd holds, behind the two seams the daemon asks
//! them through (`Accountd`: the verdicts and the key; `Reports`: a probed runtime becoming an
//! account). It is in process: inferd holds the keys itself, so nothing here is on the bus.
//! `verdicts` answers by app (an app a grant names is `Granted`, any other `Ask`, an account
//! marked denied `Denied`), and `key` hands the key of a grant. Every call is recorded (the app
//! and class asked about, the grants resolved), never a key. The real store is tested in
//! `keys_local.rs`.

use inferd::cloud::accountd::{AccountVerdict, Accountd, AccountdFault, Boxed};
use inferd::cloud::own::{OWN_PROVIDER, OwnServer};
use inferd::probe::Runtime;
use inferd::probed::Standing as Probed;
use inferd::report::{ReportFault, Reports};
use porter_core::consent::{GrantScope, Usage, Verdict};
use porter_core::{
    AccountId, AccountState, AppId, Claim, DataClass, EndpointUrl, Family, GrantId, LoginName,
    SecretText, ServiceEndpoint, Tls,
};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// What the person's account says of the apps.
#[derive(Debug, Clone)]
pub enum Standing {
    /// These apps hold this grant, and the account's key is `key` (`None`: the store cannot read
    /// it); any other app is asked.
    Granted {
        to: Vec<&'static str>,
        grant: &'static str,
        key: Option<&'static str>,
    },
    /// As `Granted`, but the grant is for one usage (`interactive` or `background`); for the
    /// other the app is asked.
    GrantedFor {
        usage: &'static str,
        to: Vec<&'static str>,
        grant: &'static str,
        key: Option<&'static str>,
    },
    /// Nobody holds a grant.
    Ask,
    /// The person refused every app.
    Denied,
}

/// One account of the fake's.
#[derive(Debug, Clone)]
pub struct FakeAccount {
    pub id: &'static str,
    pub standing: Standing,
}

/// A server of the person's own the fake holds: an account of `openai-compatible` at an address.
#[derive(Debug, Clone)]
pub struct FakeServer {
    pub id: &'static str,
    /// What the person calls it.
    pub label: &'static str,
    /// The apps that hold `grant` on it; any other is asked.
    pub to: Vec<&'static str>,
    pub grant: &'static str,
    /// Its key (`None`: the store cannot read it).
    pub key: Option<&'static str>,
    /// Its address (`https://localhost:<port>/compat/v1`).
    pub url: String,
}

/// What the fake was asked.
#[derive(Debug, Clone, Default)]
pub struct Calls {
    /// (app name, class) of each `verdicts`.
    pub verdicts: Vec<(String, String)>,
    /// The `usage` of each `verdicts`, in the same order.
    pub usages: Vec<String>,
    /// The grant of each `key`.
    pub resolved: Vec<String>,
    /// (provider, state, how many claims) of each report, in order.
    pub reported: Vec<(String, String, usize)>,
}

/// The fake, shared between the daemon and the test.
#[derive(Debug, Clone, Default)]
pub struct FakeAccountd {
    accounts: Arc<Mutex<Vec<FakeAccount>>>,
    servers: Arc<Mutex<Vec<FakeServer>>>,
    calls: Arc<Mutex<Calls>>,
    looks: Arc<Mutex<std::collections::BTreeMap<String, Look>>>,
    news: Arc<Notify>,
}

/// How the fake describes an account beyond its standing.
#[derive(Debug, Clone)]
struct Look {
    label: String,
    provider_label: Option<String>,
    state: String,
}

fn slug<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl FakeAccountd {
    /// A fake that holds these accounts.
    pub fn new(accounts: Vec<FakeAccount>) -> Self {
        Self {
            accounts: Arc::new(Mutex::new(accounts)),
            ..Self::default()
        }
    }

    /// Adds a server of the person's own, as an add sheet would.
    pub fn add_server(&self, server: FakeServer) {
        self.servers.lock().expect("lock").push(server);
        self.changed();
    }

    /// What it was asked so far.
    pub fn calls(&self) -> Calls {
        self.calls.lock().expect("lock").clone()
    }

    /// What the daemon listens to for "an account appeared, went or changed state"
    /// (`Inference::keys_news`).
    pub fn news(&self) -> Arc<Notify> {
        Arc::clone(&self.news)
    }

    /// Says that an account appeared, went or changed state, as the key host does after a save.
    pub fn changed(&self) {
        self.news.notify_one();
    }

    /// Makes the verdicts say how a person reads account `id` and whether it works: its label,
    /// its provider's name and its state slug (`ok`, `needs_reauth`).
    pub fn describe(&self, id: &str, label: &str, provider_label: Option<&str>, state: &str) {
        self.looks.lock().expect("lock").insert(
            id.to_owned(),
            Look {
                label: label.to_owned(),
                provider_label: provider_label.map(str::to_owned),
                state: state.to_owned(),
            },
        );
    }

    /// Changes what the person's account says of the apps, as a person does in Settings.
    pub fn set_standing(&self, id: &str, standing: Standing) {
        for account in self.accounts.lock().expect("lock").iter_mut() {
            if account.id == id {
                account.standing = standing.clone();
            }
        }
    }
}

impl Accountd for FakeAccountd {
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>> {
        Box::pin(async move {
            let usage_asked = slug(&usage);
            {
                let mut calls = self.calls.lock().expect("lock");
                calls
                    .verdicts
                    .push((app.name.as_str().to_owned(), slug(&class)));
                calls.usages.push(usage_asked.clone());
            }
            let looks = self.looks.lock().expect("lock").clone();
            let accounts = self.accounts.lock().expect("lock").clone();
            let servers = self.servers.lock().expect("lock").clone();
            let name = app.name.as_str();
            let own = servers.into_iter().filter_map(|server| {
                let verdict = match server.to.contains(&name) {
                    true => Verdict::Granted {
                        grant: GrantId::parse(server.grant).ok()?,
                        scope: GrantScope::Always,
                    },
                    false => Verdict::Ask,
                };
                Some(AccountVerdict {
                    account: AccountId::parse(server.id).ok()?,
                    provider: Some(OWN_PROVIDER.to_owned()),
                    label: Some(server.label.to_owned()),
                    provider_label: None,
                    state: None,
                    verdict,
                })
            });
            Ok(accounts
                .into_iter()
                .filter_map(|account| {
                    let granted = |grant: &str| {
                        GrantId::parse(grant).ok().map(|grant| Verdict::Granted {
                            grant,
                            scope: GrantScope::Always,
                        })
                    };
                    let verdict: Option<Verdict> = match &account.standing {
                        Standing::Granted { to, grant, .. } if to.contains(&name) => granted(grant),
                        Standing::GrantedFor {
                            usage: held,
                            to,
                            grant,
                            ..
                        } if *held == usage_asked && to.contains(&name) => granted(grant),
                        Standing::Granted { .. } | Standing::GrantedFor { .. } | Standing::Ask => {
                            Some(Verdict::Ask)
                        }
                        Standing::Denied => Some(Verdict::Denied),
                    };
                    let look = looks.get(account.id);
                    Some(AccountVerdict {
                        account: AccountId::parse(account.id).ok()?,
                        provider: None,
                        label: look.map(|l| l.label.clone()),
                        provider_label: look.and_then(|l| l.provider_label.clone()),
                        state: look.and_then(|l| {
                            serde_json::from_value::<AccountState>(serde_json::Value::String(
                                l.state.clone(),
                            ))
                            .ok()
                        }),
                        verdict: verdict?,
                    })
                })
                .chain(own)
                .collect())
        })
    }

    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("lock")
                .resolved
                .push(grant.as_str().to_owned());
            let accounts = self.accounts.lock().expect("lock").clone();
            let key = accounts.iter().find_map(|account| match &account.standing {
                Standing::Granted {
                    grant: held,
                    key: Some(key),
                    ..
                }
                | Standing::GrantedFor {
                    grant: held,
                    key: Some(key),
                    ..
                } if *held == grant.as_str() => Some(*key),
                _ => None,
            });
            let servers = self.servers.lock().expect("lock").clone();
            let key = key.or_else(|| {
                servers
                    .iter()
                    .find(|server| server.grant == grant.as_str())
                    .and_then(|server| server.key)
            });
            key.map(SecretText::new).ok_or(AccountdFault::Refused)
        })
    }

    fn servers(&self) -> Boxed<'_, Vec<OwnServer>> {
        Box::pin(async move {
            let servers = self.servers.lock().expect("lock").clone();
            servers
                .into_iter()
                .filter_map(|server| {
                    let endpoint = ServiceEndpoint {
                        family: Family::ChatCompletions,
                        url: EndpointUrl::parse(&server.url).ok()?,
                        tls: Tls::Implicit,
                        login: LoginName(String::new()),
                    };
                    OwnServer::of(
                        AccountId::parse(server.id).ok()?,
                        server.label.to_owned(),
                        &[endpoint],
                    )
                })
                .collect()
        })
    }

    fn server_key<'a>(
        &'a self,
        account: &'a AccountId,
    ) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move {
            let servers = self.servers.lock().expect("lock").clone();
            servers
                .iter()
                .find(|server| server.id == account.as_str())
                .and_then(|server| server.key)
                .map(SecretText::new)
                .ok_or(AccountdFault::Refused)
        })
    }
}

impl Reports for FakeAccountd {
    /// A probed runtime: recorded, and its account is the provider's id, as the real one's is.
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Probed,
    ) -> Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async move {
            let state = match standing {
                Probed::Online => "ok",
                Probed::Offline => "offline",
            };
            self.calls.lock().expect("lock").reported.push((
                runtime.provider().to_owned(),
                state.to_owned(),
                claims.len(),
            ));
            AccountId::parse(runtime.provider()).map_err(|_| ReportFault::Rejected)
        })
    }
}

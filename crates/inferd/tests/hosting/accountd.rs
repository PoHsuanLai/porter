//! A fake accountd on the private bus: `org.quire.Accounts1.Peer` at `/org/quire/Accounts1`, with
//! the two methods inferd calls. `Verdicts` answers by app (an app a grant names is `granted`, any
//! other `ask`, an account marked denied `denied`), and `ResolveKey` hands the key of a grant on a
//! sealed memfd, as the real one does. Every call is recorded (the app and class asked about, the
//! grants resolved), never a key.

use porter_dbus::{AppArg, Details, NeedArg, VerdictArg};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::io::{Seek, Write};
use std::sync::{Arc, Mutex};
use zbus::fdo;
use zbus::zvariant::{OwnedFd, OwnedValue, Value};

/// What the person's account says of the apps.
#[derive(Debug, Clone)]
pub enum Standing {
    /// These apps hold this grant, and the account's key is `key` (`None`: accountd cannot read
    /// it); any other app is asked.
    Granted {
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

/// What the fake was asked.
#[derive(Debug, Clone, Default)]
pub struct Calls {
    /// (app name, class) of each `Verdicts`.
    pub verdicts: Vec<(String, String)>,
    /// The `usage` of each `Verdicts`, in the same order.
    pub usages: Vec<String>,
    /// The grant of each `ResolveKey`.
    pub resolved: Vec<String>,
}

/// The fake, shared between the bus object and the test.
#[derive(Debug, Clone)]
pub struct FakeAccountd {
    accounts: Arc<Mutex<Vec<FakeAccount>>>,
    calls: Arc<Mutex<Calls>>,
}

fn text(value: &str) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(value.to_owned())).ok()
}

/// `key` on a memfd, sealed against every change, read position at the start.
fn sealed(key: &str) -> std::io::Result<std::os::fd::OwnedFd> {
    let fd = memfd_create("fake-key", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(key.as_bytes())?;
    file.rewind()?;
    let fd = std::os::fd::OwnedFd::from(file);
    fcntl_add_seals(
        &fd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(fd)
}

impl FakeAccountd {
    /// A fake that holds these accounts.
    pub fn new(accounts: Vec<FakeAccount>) -> Self {
        Self {
            accounts: Arc::new(Mutex::new(accounts)),
            calls: Arc::default(),
        }
    }

    /// What it was asked so far.
    pub fn calls(&self) -> Calls {
        self.calls.lock().expect("lock").clone()
    }

    /// Serves `org.quire.Accounts1` on `connection`.
    pub async fn serve(&self, connection: &zbus::Connection) {
        connection
            .object_server()
            .at("/org/quire/Accounts1", FakePeer(self.clone()))
            .await
            .expect("serve the fake peer");
        connection
            .request_name("org.quire.Accounts1")
            .await
            .expect("own org.quire.Accounts1");
    }
}

struct FakePeer(FakeAccountd);

#[zbus::interface(name = "org.quire.Accounts1.Peer")]
impl FakePeer {
    fn verdicts(
        &self,
        app: AppArg,
        _need: NeedArg,
        class: String,
        usage: String,
    ) -> fdo::Result<Vec<VerdictArg>> {
        let mut calls = self.0.calls.lock().expect("lock");
        calls.verdicts.push((app.0.clone(), class));
        calls.usages.push(usage);
        drop(calls);
        let accounts = self.0.accounts.lock().expect("lock").clone();
        Ok(accounts
            .into_iter()
            .map(|account| {
                let mut details = Details::new();
                let word = match &account.standing {
                    Standing::Granted { to, grant, .. } if to.contains(&app.0.as_str()) => {
                        details.extend(text(grant).map(|v| ("grant".to_owned(), v)));
                        details.extend(text("always").map(|v| ("scope".to_owned(), v)));
                        "granted"
                    }
                    Standing::Granted { .. } | Standing::Ask => "ask",
                    Standing::Denied => "denied",
                };
                (account.id.to_owned(), word.to_owned(), details)
            })
            .collect())
    }

    fn resolve_key(&self, grant: String) -> fdo::Result<OwnedFd> {
        self.0
            .calls
            .lock()
            .expect("lock")
            .resolved
            .push(grant.clone());
        let accounts = self.0.accounts.lock().expect("lock").clone();
        let key = accounts.iter().find_map(|account| match &account.standing {
            Standing::Granted {
                grant: held,
                key: Some(key),
                ..
            } if *held == grant => Some(*key),
            _ => None,
        });
        match key {
            Some(key) => Ok(OwnedFd::from(
                sealed(key).map_err(|e| fdo::Error::Failed(e.to_string()))?,
            )),
            None => Err(fdo::Error::AccessDenied("no such grant".into())),
        }
    }
}

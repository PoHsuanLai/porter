//! `org.quire.SettingsModule1` at `/org/quire/Accounts1/settings` (design/22 §9.4, PLAN §2.5),
//! served through quire's `ds_settings::live`. Only the `Settings` role may `Get`, `Describe`,
//! `Snapshot` or `Set`: the keys name accounts, and a `Set` removes them. The keys are in
//! `settings_keys`; this file applies them.
//!
//! Every read, `Describe`, `Get` and `Snapshot` alike, starts from one [`View`]: one copy of the
//! registry (and of the apps using each identity and the clients file) taken once, from which
//! the schema and every value are worked out. `Snapshot` gives the schema and all the values
//! from a single view, so the rows it lists and the values it carries cannot disagree, even
//! when an account is added or removed while it is being read: that is the race the default
//! (`Describe`, then a `Get` per key) leaves open.

use crate::callers::Callers;
use crate::client_rows::{RowEdit, RowRefused, edited};
use crate::host::Host;
use crate::settings_keys::{Key, UsedBy, listed, parse, schema};
use crate::state::Core;
use ds_settings::live::{
    Access, Caller, LiveError, LiveModule, LiveSchema, Snapshot, Verdict, serve,
};
use ds_settings::schema::KeyPath;
use porter_core::{AppId, AppName, AuthKind, IdentityId, Isolation};
use porter_dbus::callers::CallerRole;
use porter_dbus::names::ACCOUNTS_SETTINGS_PATH;
use porter_provider::{ClientChannel, ClientsFile, Issuer, parse_clients};
use porter_service::Registry;
use porter_service::audit::{AuditEvent, ClientIdChange};
use porter_wire::AccountsReply;
use porter_wire::ParentWindow;
use std::sync::Arc;
use zbus::Connection;

/// The settings module's path.
pub fn settings_path() -> String {
    ACCOUNTS_SETTINGS_PATH.to_owned()
}

/// The channel this build presents its client ids under.
fn channel() -> ClientChannel {
    match cfg!(debug_assertions) {
        true => ClientChannel::Development,
        false => ClientChannel::Stable,
    }
}

/// The app Settings-driven sign-ins are run as.
fn settings_app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Settings").expect("a literal app name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// How a "Sign in again" pressed in Settings ended, as the word `accounts.<id>.reauth` changes
/// to (the pane says it on the row; a refusal is no longer silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReauthNews {
    /// Signed in again.
    Done,
    /// The person closed the sheet.
    Cancelled,
    /// The service said no: the sign-in was refused or forbidden.
    Refused,
    /// It could not be done (unreachable, no client for the issuer, the store failed, ...).
    Failed,
}

impl ReauthNews {
    pub(crate) fn of(reply: &AccountsReply) -> Self {
        use porter_wire::Refusal;
        match reply {
            AccountsReply::Reauthenticated => ReauthNews::Done,
            AccountsReply::Refused(Refusal::Dismissed) => ReauthNews::Cancelled,
            AccountsReply::Refused(Refusal::Denied) => ReauthNews::Refused,
            _ => ReauthNews::Failed,
        }
    }
}

/// The module over the daemon's state.
pub(crate) struct AccountsSettings<H, C>(Arc<Core<H, C>>);

/// What one read of the module works from, taken once: the registry, the apps using each
/// identity and the person's clients file.
struct View {
    registry: Registry,
    used: UsedBy,
    clients: ClientsFile,
}

impl<H, C> AccountsSettings<H, C> {
    fn clients(&self) -> ClientsFile {
        self.0
            .clients
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| parse_clients(&text).ok())
            .unwrap_or_default()
    }
}

impl<H: Host, C> AccountsSettings<H, C> {
    /// One view of everything a read needs: one registry read, then the apps using each listed
    /// identity, then the clients file.
    async fn view(&self) -> View {
        let registry = self.0.host.registry();
        let used = {
            let uses = self.0.uses.lock().await;
            registry
                .accounts
                .iter()
                .filter(|account| listed(account, &self.0.provider_names))
                .filter_map(|account| {
                    let apps = uses.apps(&IdentityId::of(&account.id));
                    (!apps.is_empty()).then(|| (account.id.clone(), apps))
                })
                .collect()
        };
        View {
            registry,
            used,
            clients: self.clients(),
        }
    }

    /// The schema `view` lists.
    fn schema_of(&self, view: &View) -> LiveSchema {
        schema(
            &view.registry,
            &view.used,
            &self.0.app_names,
            &self.0.provider_names,
        )
    }

    /// The value of the key at `path` in `view`.
    fn value_in(&self, view: &View, path: &KeyPath) -> Result<toml::Value, LiveError> {
        let registry = &view.registry;
        let unknown = || LiveError::UnknownKey(path.0.clone());
        let account = |id: &porter_core::AccountId| {
            registry
                .accounts
                .iter()
                .find(|a| a.id == *id)
                .ok_or_else(unknown)
        };
        let text = |text: &str| Ok(toml::Value::String(text.to_owned()));
        match parse(&path.0, registry).ok_or_else(unknown)? {
            Key::State(id) => text(crate::state::state_slug(account(&id)?.state)),
            Key::Label(id) => text(&account(&id)?.label.0),
            Key::Expires(id) => crate::settings_keys::expiry_text(account(&id)?)
                .map(toml::Value::String)
                .ok_or_else(unknown),
            Key::Place(id) => text(crate::settings_keys::place_slug(account(&id)?)),
            Key::SignIn(id) => text(account(&id)?.auth.facts().way.slug()),
            Key::Provider(id) => text(account(&id)?.provider.as_str()),
            Key::Group(id) => text(self.0.provider_names.group_of(account(&id)?).slug()),
            Key::UsedBy(id, n) => view
                .used
                .get(&id)
                .and_then(|apps| apps.get(n))
                .map(|app| toml::Value::String(self.0.app_names.title_of(app).0))
                .ok_or_else(unknown),
            Key::Reauth(_) | Key::Remove(_) => Ok(toml::Value::Boolean(false)),
            Key::Client(issuer) => text(
                &view
                    .clients
                    .clients
                    .iter()
                    .find(|c| c.issuer == issuer && c.channel == channel())
                    .map(|c| c.client_id.0.clone())
                    .unwrap_or_default(),
            ),
            // The application secret is the same for every user of the client and is no
            // person's credential (Google says so of desktop clients): Settings shows it back.
            Key::ClientSecret(issuer) => text(
                &view
                    .clients
                    .clients
                    .iter()
                    .find(|c| c.issuer == issuer && c.channel == channel())
                    .and_then(|c| c.client_secret.as_ref())
                    .map(|s| s.expose().to_owned())
                    .unwrap_or_default(),
            ),
        }
    }
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

impl<H: Host, C: Callers> LiveModule for AccountsSettings<H, C> {
    async fn permit(&self, caller: &Caller, _access: Access) -> Verdict {
        match self.0.callers.caller_of(&caller.sender.0).await {
            Some(who) if who.role == CallerRole::Settings => Verdict::Allow,
            _ => Verdict::Refuse,
        }
    }

    async fn describe(&self) -> LiveSchema {
        self.schema_of(&self.view().await)
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        self.value_in(&self.view().await, key)
    }

    /// The schema and every value from one view: one registry read, so a row and its value
    /// cannot disagree over an account that came or went between two reads.
    async fn snapshot(&self) -> Result<Snapshot, LiveError> {
        let view = self.view().await;
        let schema = self.schema_of(&view);
        let reads: Vec<_> = schema
            .key
            .iter()
            .map(|spec| (spec.path.clone(), self.value_in(&view, &spec.path)))
            .collect();
        Snapshot::from_reads(schema, reads)
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        let registry = self.0.host.registry();
        let parsed =
            parse(&key.0, &registry).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        match parsed {
            Key::Remove(id) => {
                self.0.host.remove(&id).await.map_err(failed)?;
            }
            Key::Reauth(id) => {
                // A program on this computer has nothing to sign in to (the row is not offered).
                let local = registry.accounts.iter().any(|a| {
                    a.id == id && matches!(a.auth, AuthKind::LocalRuntime | AuthKind::None)
                });
                if local {
                    return Err(LiveError::NotPermitted(
                        "this account has nothing to sign in to".into(),
                    ));
                }
                let core = Arc::clone(&self.0);
                tokio::spawn(async move {
                    let reply = core
                        .host
                        .reauthenticate_any(&settings_app(), &id, ParentWindow::Unparented)
                        .await;
                    if let AccountsReply::Refused(why) = &reply {
                        eprintln!("accountd: reauthenticate from Settings refused: {why:?}");
                    }
                    // The pane hears how it ended on the row.
                    announce_row(
                        &core,
                        &Key::Reauth(id),
                        porter_ids::word::word(&ReauthNews::of(&reply)),
                    )
                    .await;
                    core.publish().await;
                });
            }
            Key::Client(issuer) => {
                let text = match &value {
                    toml::Value::String(text) => text.trim().to_owned(),
                    _ => return Err(LiveError::BadValue("a sign-in key is text".into())),
                };
                self.write_client(issuer, RowEdit::Id(&text))?;
                let slug = serde_json::to_value(issuer)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                self.0.host.audit_settings(AuditEvent::ClientIdChanged {
                    issuer: slug,
                    change: match text.is_empty() {
                        true => ClientIdChange::Cleared,
                        false => ClientIdChange::Set,
                    },
                });
            }
            Key::ClientSecret(issuer) => {
                let text = match &value {
                    toml::Value::String(text) => text.trim().to_owned(),
                    _ => return Err(LiveError::BadValue("a sign-in secret is text".into())),
                };
                self.write_client(issuer, RowEdit::Secret(&text))?;
            }
            Key::State(_)
            | Key::Label(_)
            | Key::Place(_)
            | Key::SignIn(_)
            | Key::Expires(_)
            | Key::Provider(_)
            | Key::Group(_)
            | Key::UsedBy(..) => {
                return Err(LiveError::NotPermitted("this row is a read-out".into()));
            }
        }
        self.0.publish().await;
        Ok(())
    }
}

impl<H, C> AccountsSettings<H, C> {
    /// Writes `issuer`'s bring-your-own client id (empty removes the row) or its secret to the
    /// user's clients file, atomically; Settings is the only writer (PLAN §2.6). The row's other
    /// lines (Google's `testing`, `byo`) and every other row are kept as they were.
    fn write_client(&self, issuer: Issuer, edit: RowEdit<'_>) -> Result<(), LiveError> {
        let path = self
            .0
            .clients
            .as_deref()
            .ok_or_else(|| failed("no clients file is configured"))?;
        let before = std::fs::read_to_string(path).unwrap_or_default();
        let text = edited(&before, issuer, channel(), edit).map_err(|refused| match refused {
            RowRefused::NoKey => {
                LiveError::BadValue("Set the sign-in key first, then its secret.".into())
            }
            RowRefused::Unwritable(why) => failed(why),
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let temp = path.with_extension("toml.new");
        // On disk before the rename publishes it, or a power cut leaves an empty clients file.
        std::fs::File::create(&temp)
            .and_then(|mut out| {
                use std::io::Write;
                out.write_all(text.as_bytes())?;
                out.sync_all()
            })
            .map_err(failed)?;
        std::fs::rename(&temp, path).map_err(failed)
    }
}

/// Serves the module at [`settings_path`].
pub(crate) async fn serve_settings<H: Host, C: Callers>(
    connection: &Connection,
    core: &Arc<Core<H, C>>,
) -> zbus::Result<()> {
    let served = serve(
        connection,
        ACCOUNTS_SETTINGS_PATH,
        AccountsSettings(Arc::clone(core)),
    )
    .await
    .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    // Served once; a second call leaves the first in place.
    let _ = core.settings.set(served);
    Ok(())
}

/// Tells the settings module's listeners `Changed(<the key's path>, <word>)`: how an action
/// row's action went.
async fn announce_row<H, C>(core: &Core<H, C>, key: &Key, word: &str) {
    if let Some(module) = core.settings.get() {
        let _ = module
            .changed(
                &KeyPath(crate::settings_keys::path(key)),
                &toml::Value::String(word.to_owned()),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_ids::word::word;

    #[test]
    fn each_reauth_news_keeps_the_word_the_pane_reads() {
        assert_eq!(word(&ReauthNews::Done), "done");
        assert_eq!(word(&ReauthNews::Cancelled), "cancelled");
        assert_eq!(word(&ReauthNews::Refused), "refused");
        assert_eq!(word(&ReauthNews::Failed), "failed");
    }
}

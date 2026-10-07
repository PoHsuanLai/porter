//! `org.quire.SettingsModule1` at `/org/quire/Accounts1/settings` (design/22 §9.4, PLAN §2.5),
//! served through quire's `ds_settings::live`. Only the `Settings` role may `Get`, `Describe` or
//! `Set`: the keys name accounts and grants, and a `Set` removes accounts. The keys are in
//! `settings_keys`; this file applies them.

use crate::callers::Callers;
use crate::core::{Core, Host};
use crate::settings_keys::{Key, parse, schema};
use ds_settings::live::{Access, Caller, LiveError, LiveModule, LiveSchema, Verdict, serve};
use ds_settings::schema::KeyPath;
use porter_core::wire::ParentWindow;
use porter_core::{AccountsReply, AppId, AppName, Isolation, Toggle};
use porter_dbus::{ACCOUNTS_SETTINGS_PATH, CallerRole};
use porter_provider::{ClientChannel, ClientEntry, ClientId, ClientsFile, Issuer, parse_clients};
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

/// The module over the daemon's state.
pub(crate) struct AccountsSettings<H, C>(Arc<Core<H, C>>);

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

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

fn word(value: &toml::Value) -> Option<Toggle> {
    match value {
        toml::Value::String(s) if s == "on" => Some(Toggle::On),
        toml::Value::String(s) if s == "off" => Some(Toggle::Off),
        toml::Value::Boolean(true) => Some(Toggle::On),
        toml::Value::Boolean(false) => Some(Toggle::Off),
        _ => None,
    }
}

impl<H: Host, C: Callers> LiveModule for AccountsSettings<H, C> {
    async fn permit(&self, caller: &Caller, _access: Access) -> Verdict {
        match self.0.callers.caller_of(&caller.sender.0).await {
            Some(who) if who.role == CallerRole::Settings => Verdict::Allow,
            _ => Verdict::Refuse,
        }
    }

    async fn describe(&self) -> LiveSchema {
        schema(&self.0.host.registry())
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        let registry = self.0.host.registry();
        let unknown = || LiveError::UnknownKey(key.0.clone());
        let account = |id: &porter_core::AccountId| {
            registry
                .accounts
                .iter()
                .find(|a| a.id == *id)
                .ok_or_else(unknown)
        };
        match parse(&key.0, &registry).ok_or_else(unknown)? {
            Key::Service(id, kind) => {
                let on = account(&id)?.capabilities.iter().any(|c| {
                    c.offer.kind() == kind && matches!(c.offer, porter_core::Offer::Present(_))
                });
                Ok(toml::Value::String(
                    if on { "on" } else { "off" }.to_owned(),
                ))
            }
            Key::Sync(id, class) => {
                account(&id)?;
                let on = porter_service::sync_allowed(&registry.grants, &id, class);
                Ok(toml::Value::String(
                    if on { "on" } else { "off" }.to_owned(),
                ))
            }
            Key::State(id) => Ok(toml::Value::String(
                crate::account::state_slug(account(&id)?.state).to_owned(),
            )),
            Key::Label(id) => Ok(toml::Value::String(account(&id)?.label.0.clone())),
            Key::Place(id) => Ok(toml::Value::String(
                crate::settings_keys::place_slug(account(&id)?).to_owned(),
            )),
            Key::Grant(..) | Key::Reauth(_) | Key::Remove(_) => Ok(toml::Value::Boolean(false)),
            Key::Client(issuer) => Ok(toml::Value::String(
                self.clients()
                    .clients
                    .iter()
                    .find(|c| c.issuer == issuer && c.channel == channel())
                    .map(|c| c.client_id.0.clone())
                    .unwrap_or_default(),
            )),
        }
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        let registry = self.0.host.registry();
        let parsed =
            parse(&key.0, &registry).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        let refused = |r: porter_core::wire::Refusal| failed(format!("refused: {r:?}"));
        match parsed {
            Key::Service(id, kind) => {
                let toggle = word(&value)
                    .ok_or_else(|| LiveError::BadValue("a service is `on` or `off`".into()))?;
                self.0
                    .host
                    .set_toggle(&id, kind, toggle)
                    .await
                    .map_err(refused)?;
            }
            Key::Sync(id, class) => {
                let toggle = word(&value)
                    .ok_or_else(|| LiveError::BadValue("a sync row is `on` or `off`".into()))?;
                self.0
                    .host
                    .set_sync(&id, class, toggle)
                    .await
                    .map_err(refused)?;
            }
            Key::Grant(_, grant) => {
                if !registry.grants.iter().any(|g| g.id == grant) {
                    return Err(LiveError::UnknownKey(key.0.clone()));
                }
                self.0.host.revoke_grant(&grant).await.map_err(refused)?;
            }
            Key::Remove(id) => {
                self.0.host.remove(&id).await.map_err(failed)?;
            }
            Key::Reauth(id) => {
                let core = Arc::clone(&self.0);
                tokio::spawn(async move {
                    if let AccountsReply::Refused(why) = core
                        .host
                        .reauthenticate_any(&settings_app(), &id, ParentWindow::Unparented)
                        .await
                    {
                        eprintln!("accountd: reauthenticate from Settings refused: {why:?}");
                    }
                    core.publish().await;
                });
            }
            Key::Client(issuer) => {
                let text = match &value {
                    toml::Value::String(text) => text.trim().to_owned(),
                    _ => return Err(LiveError::BadValue("a client id is text".into())),
                };
                self.write_client(issuer, &text)?;
            }
            Key::State(_) | Key::Label(_) | Key::Place(_) => {
                return Err(LiveError::NotPermitted("this row is a read-out".into()));
            }
        }
        self.0.publish().await;
        Ok(())
    }
}

impl<H, C> AccountsSettings<H, C> {
    /// Writes `issuer`'s bring-your-own client id (empty removes it) to the user's clients file,
    /// atomically; Settings is the only writer (PLAN §2.6).
    fn write_client(&self, issuer: Issuer, client_id: &str) -> Result<(), LiveError> {
        let path = self
            .0
            .clients
            .as_deref()
            .ok_or_else(|| failed("no clients file is configured"))?;
        let mut file = self.clients();
        file.clients
            .retain(|c| !(c.issuer == issuer && c.channel == channel()));
        if !client_id.is_empty() {
            file.clients.push(ClientEntry {
                issuer,
                channel: channel(),
                client_id: ClientId(client_id.to_owned()),
                client_secret: None,
                endpoints: None,
            });
        }
        let text = toml::to_string(&file).map_err(failed)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let temp = path.with_extension("toml.new");
        std::fs::write(&temp, text).map_err(failed)?;
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

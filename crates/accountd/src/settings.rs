//! `org.quire.SettingsModule1` at `/org/quire/Accounts1/settings` (design/22 §9.4, PLAN §2.5),
//! served through quire's `ds_settings::live`. Only the `Settings` role may `Get`, `Describe`,
//! `Snapshot` or `Set`: the keys name accounts, and a `Set` removes them. The keys are in
//! `porter-settings`; this file hosts them and applies what is written.
//!
//! Every read, `Describe`, `Get` and `Snapshot` alike, starts from one [`View`]: one copy of the
//! registry (and of the apps using each identity and the clients file) taken once, from which
//! the schema and every value are worked out. `Snapshot` gives the schema and all the values
//! from a single view, so the rows it lists and the values it carries cannot disagree, even
//! when an account is added or removed while it is being read: that is the race the default
//! (`Describe`, then a `Get` per key) leaves open.

use crate::callers::Callers;
use crate::host::Host;
use crate::launcher::SignOutNews;
use crate::state::Core;
use ds_settings::live::{
    Access, Caller, LiveError, LiveModule, LiveSchema, Snapshot, Verdict, serve,
};
use ds_settings::schema::KeyPath;
use porter_callers::CallerRole;
use porter_capability::{CapabilityKind, Toggle};
use porter_core::ProviderId;
use porter_core::{
    Account, AccountId, AgentState, AppId, AppName, AuthKind, GrantId, IdentityId, Isolation,
};
use porter_dbus::accounts::ACCOUNTS_SETTINGS_PATH;
use porter_families::set::ProviderNames;
use porter_identities::IdentitiesApi;
use porter_provider::{ClientChannel, ClientsFile, Issuer, parse_clients};
use porter_service_accounts::{Launchers as _, program_of};
use porter_service_core::audit::{AuditEvent, ClientIdChange};
use porter_service_core::registry::Registry;
use porter_settings::accounts::{Directory, Key, UsedBy, listed, parse, read, schema};
use porter_settings::clients::{RowEdit, RowRefused, edited};
use porter_settings::{Acts, At, Boxed, REAUTH, SIGN_OUT};
use porter_sheet::ProviderGroup;
use porter_sheets::AppNames;
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
    /// An assistant's login was asked for and no launcher runs that program.
    NoLauncher,
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
            AccountsReply::Refused(Refusal::NoLauncher) => ReauthNews::NoLauncher,
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
                .filter(|account| listed(account, &self.directory()))
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
        schema(&view.registry, &view.used, &self.directory())
    }

    /// How the daemon's provider and app names read to the rows.
    fn directory(&self) -> Names<'_> {
        Names {
            apps: &self.0.app_names,
            providers: &self.0.provider_names,
        }
    }

    /// The value of the key at `path` in `view`.
    fn value_in(&self, view: &View, path: &KeyPath) -> Result<toml::Value, LiveError> {
        let unknown = || LiveError::UnknownKey(path.0.clone());
        let text = |text: &str| Ok(toml::Value::String(text.to_owned()));
        let key = parse(&path.0, &view.registry).ok_or_else(unknown)?;
        match key {
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
            key => read(&key, &view.registry, &view.used, &self.directory()).ok_or_else(unknown),
        }
    }
}

/// What the rows of `porter-settings` ask of the daemon: how a provider and an app are called.
struct Names<'a> {
    apps: &'a AppNames,
    providers: &'a ProviderNames,
}

impl Directory for Names<'_> {
    fn provider_label(&self, provider: &ProviderId) -> Option<&str> {
        self.providers.label_of(provider)
    }

    fn group_of(&self, account: &Account) -> ProviderGroup {
        self.providers.group_of(account)
    }

    fn app_title(&self, app: &AppName) -> String {
        self.apps.title_of(app).0
    }
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

/// A refusal of a change made from Settings: logged, and told to the pane in plain words.
fn refused(refusal: porter_wire::Refusal) -> LiveError {
    eprintln!("accountd: a Settings change was refused: {refusal:?}");
    failed("That could not be done.")
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
        let unknown = || LiveError::UnknownKey(key.0.clone());
        match parse(&key.0, &registry).ok_or_else(unknown)? {
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
            Key::Account(id, row, tail) => {
                let account = registry
                    .accounts
                    .iter()
                    .find(|a| a.id == id)
                    .ok_or_else(unknown)?;
                let (directory, used) = (self.directory(), UsedBy::new());
                let at = At::of(account, &registry, &used, &directory);
                row.0.write(&tail, value, &at, self).await?;
            }
        }
        self.0.publish().await;
        Ok(())
    }
}

/// What the action rows do, over the daemon's state.
impl<H: Host, C: Callers> Acts for AccountsSettings<H, C> {
    fn remove<'a>(&'a self, id: &'a AccountId) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(async move {
            self.0.host.remove(id).await.map_err(failed)?;
            Ok(())
        })
    }

    /// An assistant signs itself in, through its launcher, with a sheet that shows the wait; any
    /// other account is signed in again through its provider. The pane hears how it ended on the
    /// row.
    fn sign_in_again<'a>(&'a self, account: &'a Account) -> Boxed<'a, Result<(), LiveError>> {
        let core = Arc::clone(&self.0);
        let id = account.id.clone();
        let agent = account.auth == AuthKind::AgentLogin;
        Box::pin(async move {
            tokio::spawn(async move {
                let reply = match agent {
                    true => {
                        core.host
                            .login_agent_any(
                                &settings_app(),
                                &id,
                                ParentWindow::Unparented,
                                &core.launchers,
                            )
                            .await
                    }
                    false => {
                        core.host
                            .reauthenticate_any(&settings_app(), &id, ParentWindow::Unparented)
                            .await
                    }
                };
                if let AccountsReply::Refused(why) = &reply {
                    eprintln!("accountd: sign in again from Settings refused: {why:?}");
                }
                announce_row(
                    &core,
                    &Key::row(id, REAUTH),
                    porter_ids::word::word(&ReauthNews::of(&reply)),
                )
                .await;
                core.publish().await;
            });
            Ok(())
        })
    }

    fn sign_out<'a>(&'a self, id: &'a AccountId) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(self.sign_out_of(id))
    }

    fn switch<'a>(
        &'a self,
        id: &'a AccountId,
        kind: CapabilityKind,
        to: Toggle,
    ) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(async move { self.0.host.set_toggle(id, kind, to).await.map_err(refused) })
    }

    fn revoke<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<(), LiveError>> {
        Box::pin(async move { self.0.host.revoke_grant(grant).await.map_err(refused) })
    }
}

impl<H: Host, C: Callers> AccountsSettings<H, C> {
    /// "Sign out" of an assistant: the launcher that runs its program is asked to have it sign
    /// itself out, and the pane hears how that ended; with no launcher, accountd only forgets
    /// that it was signed in and the assistant's own login is not touched.
    async fn sign_out_of(&self, id: &AccountId) -> Result<(), LiveError> {
        let core = &self.0;
        let program = core
            .host
            .registry()
            .accounts
            .iter()
            .find(|a| a.id == *id && a.auth == AuthKind::AgentLogin)
            .and_then(program_of);
        let waiting = match program {
            Some(program) => core.launchers.ask_logout(id, &program).await.ok(),
            None => None,
        };
        let key = Key::row(id.clone(), SIGN_OUT);
        let Some(waiting) = waiting else {
            core.host
                .set_agent_state(id, AgentState::NeedsLogin)
                .await
                .map_err(|fault| {
                    eprintln!("accountd: signing an assistant out was refused: {fault:?}");
                    failed("That could not be done.")
                })?;
            announce_row(core, &key, SignOutNews::LoginUntouched.slug()).await;
            core.publish().await;
            return Ok(());
        };
        announce_row(core, &key, SignOutNews::Asked.slug()).await;
        let core = Arc::clone(core);
        tokio::spawn(async move {
            let news = SignOutNews::of(waiting.await);
            announce_row(&core, &key, news.slug()).await;
            core.publish().await;
        });
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
                &KeyPath(porter_settings::accounts::path(key)),
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
        assert_eq!(word(&ReauthNews::NoLauncher), "no_launcher");
        assert_eq!(word(&ReauthNews::Failed), "failed");
    }
}

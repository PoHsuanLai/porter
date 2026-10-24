//! `org.quire.SettingsModule1` at `/org/quire/Sync1/settings`, served through quire's
//! `ds_settings::live`: the accounts syncd signed in for itself and what it keeps of them. Only the
//! `Settings` role may `Get`, `Describe` or `Set`, as in accountd's module.
//!
//! One group of rows per account syncd holds: `accounts.<id>.label`, `.provider` and `.state`
//! (read-outs), `.sync.files` and `.sync.photos` (what syncd keeps of the account, shown for what
//! it offers), `.storage.files` ("Show in Files", for an account whose provider asks the whole
//! drive on use), `.reauth` ("Sign in again") and `.remove`. Turning "Show in Files" on may open
//! the provider's page, so it answers at once and the row says how it went afterwards: on, or
//! off with a line under it when the person did not give the access. One more row, `accounts.sync.add`, starts
//! "Continue as": the person picks an account they are signed in to and syncd signs in as it. The
//! switch for a new account turns on only once that sign-in finished. Removing an account deletes
//! its journals and mirrors here and its stored sign-in; accountd's own removal of the identity
//! does not touch it. One more row, `storage.cache_limit` ("Downloaded Files Limit", an Advanced
//! text row of bytes, "0" for automatic), is how much the drives shown in Files keep of what is
//! not kept downloaded; it is written to `syncd.toml` and every drive follows it at once. No key
//! is agent-settable.

use crate::datasets::drive::CacheKnob;
use crate::keep::{
    ContinueError, Directory, Hosted, Keep, SyncClass, account_of, drive_allowed, sync_allowed,
    sync_offers,
};
use ds_settings::live::{
    Access, Caller, LiveError, LiveModule, LiveSchema, Served, Verdict, serve,
};
use ds_settings::schema::{
    ActionLabel, ActionWeight, AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label,
    LiveAction, Note, Page, Section, WordLabels,
};
use porter_core::wire::ParentWindow;
use porter_core::{Account, AccountId, AccountState, Family, Toggle};
use porter_dbus::{CallerRole, Callers};
use porter_drive::CacheLimit;
use porter_families::set::ProviderNames;
use porter_service::Registry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use zbus::Connection;

/// The settings module's path.
pub const SETTINGS_PATH: &str = "/org/quire/Sync1/settings";

/// What a key names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// `accounts.sync.add`: "Continue as" for a new account.
    Add,
    /// `accounts.<id>.label`.
    Label(AccountId),
    /// `accounts.<id>.provider`.
    Provider(AccountId),
    /// `accounts.<id>.state`.
    State(AccountId),
    /// `accounts.<id>.sync.files` and `.sync.photos`.
    Sync(AccountId, SyncClass),
    /// `accounts.<id>.storage.files`: "Show in Files".
    Files(AccountId),
    /// `accounts.<id>.reauth`.
    Reauth(AccountId),
    /// `accounts.<id>.remove`.
    Remove(AccountId),
    /// `storage.cache_limit`.
    CacheLimit,
}

const ADD: &str = "accounts.sync.add";

/// The cache limit's row.
pub const CACHE_LIMIT: &str = "storage.cache_limit";

/// The cache limit's label and help line.
const CACHE_LIMIT_WORDS: (&str, &str) = (
    "Downloaded Files Limit",
    "Files you haven't kept downloaded are removed, oldest first, past this size.",
);

/// The key `path` names, among the accounts of `registry` (an id may hold dots, so the account is
/// found by prefix, the longest id first).
pub(crate) fn parse(path: &str, registry: &Registry) -> Option<Key> {
    if path == ADD {
        return Some(Key::Add);
    }
    if path == CACHE_LIMIT {
        return Some(Key::CacheLimit);
    }
    let rest = path.strip_prefix("accounts.")?;
    let mut ids: Vec<&AccountId> = registry.accounts.iter().map(|a| &a.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_str().len()));
    ids.into_iter().find_map(|id| {
        let tail = rest.strip_prefix(id.as_str())?.strip_prefix('.')?;
        let id = id.clone();
        match tail {
            "label" => Some(Key::Label(id)),
            "provider" => Some(Key::Provider(id)),
            "state" => Some(Key::State(id)),
            "reauth" => Some(Key::Reauth(id)),
            "remove" => Some(Key::Remove(id)),
            "storage.files" => Some(Key::Files(id)),
            _ => {
                let what = tail.strip_prefix("sync.")?;
                SyncClass::ALL
                    .into_iter()
                    .find(|c| c.slug() == what)
                    .map(|c| Key::Sync(id, c))
            }
        }
    })
}

/// The path of `key`.
pub(crate) fn path(key: &Key) -> String {
    match key {
        Key::Add => ADD.to_owned(),
        Key::CacheLimit => CACHE_LIMIT.to_owned(),
        Key::Label(id) => format!("accounts.{id}.label"),
        Key::Provider(id) => format!("accounts.{id}.provider"),
        Key::State(id) => format!("accounts.{id}.state"),
        Key::Sync(id, class) => format!("accounts.{id}.sync.{}", class.slug()),
        Key::Files(id) => format!("accounts.{id}.storage.files"),
        Key::Reauth(id) => format!("accounts.{id}.reauth"),
        Key::Remove(id) => format!("accounts.{id}.remove"),
    }
}

fn state_slug(state: AccountState) -> &'static str {
    match state {
        AccountState::Ok => "ok",
        AccountState::NeedsReauth => "needs_reauth",
        AccountState::Offline => "offline",
        AccountState::Limited => "limited",
        AccountState::NeedsLogin => "needs_login",
    }
}

fn spec(
    key: &Key,
    section: &str,
    label: &str,
    help: &str,
    kind: KeyKind,
    default: toml::Value,
) -> KeySpec {
    KeySpec {
        control: None,
        path: KeyPath(path(key)),
        kind,
        default,
        label: Label(label.to_owned()),
        help: Help(help.to_owned()),
        page: Page::Accounts,
        section: Section(section.to_owned()),
        exposure: Exposure::Basic,
        labels: WordLabels::default(),
        unavailable: Default::default(),
        groups: Default::default(),
        places: Default::default(),
        note: None,
        agent: AgentSetting::HandsOff,
    }
}

fn action(name: &str, weight: ActionWeight) -> KeyKind {
    KeyKind::Live {
        action: LiveAction {
            label: ActionLabel(name.to_owned()),
            weight,
        },
    }
}

fn readout(key: &Key, section: &str, label: &str, word: &str) -> KeySpec {
    spec(
        key,
        section,
        label,
        "",
        KeyKind::Fixed {
            variant: word.to_owned(),
        },
        toml::Value::String(word.to_owned()),
    )
}

fn off() -> toml::Value {
    toml::Value::Boolean(false)
}

/// What a switch says about a class, in plain words.
fn sync_words(account: &Account, class: SyncClass) -> (&'static str, &'static str) {
    match class {
        SyncClass::Files => (
            "Keep this account's files on this computer",
            "Lets the sync service keep a copy of this account's app folder here.",
        ),
        // Google Photos is not a backup of a library: it is an upload folder, and what the
        // person picks in Google's own window. The row says so.
        SyncClass::Photos
            if account
                .endpoints
                .iter()
                .any(|e| e.family == Family::GooglePhotosUpload) =>
        {
            (
                "Upload new photos from a folder on this computer to Google Photos",
                "Sends each new photo you put in this account's upload folder, a folder on this computer, to an album made for it in Google Photos. It never reads or deletes anything else in your Google Photos.",
            )
        }
        SyncClass::Photos => (
            "Back up photos",
            "Lets the sync service keep this account's photos here and back them up.",
        ),
    }
}

/// "Show in Files": its label and help line.
const FILES_WORDS: (&str, &str) = (
    "Show in Files",
    "See this account's files in Files. They download when you open them.",
);

/// The line under "Show in Files" when turning it on did not end in it on.
pub const FILES_NOT_ON: &str = "Files wasn't turned on. You can try again.";

/// How an account's rows are drawn beyond the registry: whether "Show in Files" is offered, and
/// whether its last turning on did not go through.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FilesRow {
    pub offered: bool,
    pub not_on: bool,
}

fn account_keys(account: &Account, names: &ProviderNames, files: FilesRow) -> Vec<KeySpec> {
    let section = account.label.0.as_str();
    let id = &account.id;
    let provider = names
        .label_of(&account.provider)
        .unwrap_or_else(|| account.provider.as_str());
    let mut provider_row = readout(
        &Key::Provider(id.clone()),
        section,
        "Account type",
        account.provider.as_str(),
    );
    provider_row.labels = WordLabels(
        [(account.provider.as_str().to_owned(), provider.to_owned())]
            .into_iter()
            .collect(),
    );
    let mut keys = vec![
        readout(
            &Key::Label(id.clone()),
            section,
            "Account",
            &account.label.0,
        ),
        provider_row,
        readout(
            &Key::State(id.clone()),
            section,
            "Status",
            state_slug(account.state),
        ),
    ];
    for class in sync_offers(account) {
        let (label, help) = sync_words(account, class);
        keys.push(spec(
            &Key::Sync(id.clone(), class),
            section,
            label,
            help,
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("off".to_owned()),
        ));
    }
    if files.offered {
        let (label, help) = FILES_WORDS;
        let mut row = spec(
            &Key::Files(id.clone()),
            section,
            label,
            help,
            KeyKind::Toggle {
                variants: ["on".to_owned(), "off".to_owned()],
            },
            toml::Value::String("off".to_owned()),
        );
        row.note = files.not_on.then(|| Note(FILES_NOT_ON.to_owned()));
        keys.push(row);
    }
    keys.push(spec(
        &Key::Reauth(id.clone()),
        section,
        "Sign in again",
        "",
        action("Sign in again", ActionWeight::Plain),
        off(),
    ));
    keys.push(spec(
        &Key::Remove(id.clone()),
        section,
        "Remove account",
        "Stops syncing this account, deletes its copies on this computer and its stored sign-in here. The account itself, and its sign-in in your other accounts, stay.",
        action("Remove account", ActionWeight::Destructive),
        off(),
    ));
    keys
}

/// The schema of the module for `registry`, each account's "Show in Files" row as `files` says.
pub(crate) fn schema(
    registry: &Registry,
    names: &ProviderNames,
    files: impl Fn(&Account) -> FilesRow,
) -> LiveSchema {
    let mut key = vec![spec(
        &Key::Add,
        "Sync",
        "Add an account to sync",
        "Pick one of the accounts you are signed in to on this computer. Sync signs in as it and keeps its files here.",
        action("Add Account", ActionWeight::Plain),
        off(),
    )];
    key.extend(
        registry
            .accounts
            .iter()
            .flat_map(|account| account_keys(account, names, files(account))),
    );
    let (label, help) = CACHE_LIMIT_WORDS;
    let mut limit = spec(
        &Key::CacheLimit,
        "Files",
        label,
        help,
        KeyKind::Text,
        toml::Value::String("0".to_owned()),
    );
    limit.exposure = Exposure::Advanced;
    key.push(limit);
    LiveSchema { version: 1, key }
}

/// How an action ended, as the word its row changes to.
fn word_of(outcome: &Result<(), ContinueError>) -> &'static str {
    match outcome {
        Ok(()) => "done",
        Err(ContinueError::Cancelled) => "cancelled",
        Err(ContinueError::NoIdentity) => "no_identity",
        Err(_) => "failed",
    }
}

fn failed(why: impl ToString) -> LiveError {
    LiveError::Failed(why.to_string())
}

fn toggle_of(value: &toml::Value) -> Option<Toggle> {
    match value {
        toml::Value::String(s) if s == "on" => Some(Toggle::On),
        toml::Value::String(s) if s == "off" => Some(Toggle::Off),
        toml::Value::Boolean(true) => Some(Toggle::On),
        toml::Value::Boolean(false) => Some(Toggle::Off),
        _ => None,
    }
}

/// The module over syncd's accounts.
pub struct SyncSettings<H, D, C> {
    keep: Arc<Keep<H, D>>,
    callers: Arc<C>,
    names: ProviderNames,
    served: Arc<OnceLock<Served>>,
    /// The accounts whose "Show in Files" was last turned on without it ending on.
    not_on: Arc<Mutex<BTreeSet<AccountId>>>,
    /// The cache limit the drives follow, and the file it is kept in.
    cache: Option<(CacheKnob, PathBuf)>,
}

impl<H, D, C> std::fmt::Debug for SyncSettings<H, D, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSettings").finish_non_exhaustive()
    }
}

impl<H, D, C> SyncSettings<H, D, C> {
    /// The module over `keep`, deciding who may use it through `callers`, naming providers from
    /// `names`.
    pub fn new(keep: Arc<Keep<H, D>>, callers: Arc<C>, names: ProviderNames) -> Self {
        Self {
            keep,
            callers,
            names,
            served: Arc::new(OnceLock::new()),
            not_on: Arc::default(),
            cache: None,
        }
    }

    /// With the cache limit row: setting it writes `file` and moves `knob`, which the drives
    /// follow.
    #[must_use]
    pub fn with_cache_limit(mut self, knob: CacheKnob, file: PathBuf) -> Self {
        self.cache = Some((knob, file));
        self
    }
}

/// Where syncd kept its settings before they had a folder of their own: the same name one folder
/// up (`$XDG_CONFIG_HOME/porter/syncd.toml` for `porter/syncd/syncd.toml`).
fn earlier(file: &Path) -> Option<PathBuf> {
    Some(file.parent()?.parent()?.join(file.file_name()?))
}

/// The text of the settings file `file`; while there is none, the one kept where it was before
/// ([`earlier`]). That one is only read: the next save writes `file`, and the earlier one stays
/// where it is, since syncd's unit may write only syncd's own folder and so cannot remove it.
fn settings_text(file: &Path) -> Option<String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Some(text),
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
            std::fs::read_to_string(earlier(file)?).ok()
        }
        Err(_) => None,
    }
}

/// The cache limit `file` keeps (`[storage] cache_limit = "<bytes>"`); automatic when there is
/// no file, or it says nothing this build reads.
pub fn read_cache_limit(file: &Path) -> CacheLimit {
    let Some(text) = settings_text(file) else {
        return CacheLimit::Automatic;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return CacheLimit::Automatic;
    };
    table
        .get("storage")
        .and_then(|storage| storage.get("cache_limit"))
        .and_then(bytes_of)
        .map_or(CacheLimit::Automatic, CacheLimit::from_setting)
}

/// Writes `limit` to `file`, keeping the rest of it.
fn write_cache_limit(file: &Path, limit: CacheLimit) -> std::io::Result<()> {
    let mut table = settings_text(file)
        .and_then(|text| text.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let storage = table
        .entry("storage")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(storage) = storage {
        storage.insert(
            "cache_limit".to_owned(),
            toml::Value::String(limit.setting().to_string()),
        );
    }
    let text = toml::to_string(&table).map_err(std::io::Error::other)?;
    porter_fs::atomic::AtomicWrite::PRIVATE.write(file, text.as_bytes())
}

/// Bytes as the row takes them: text of digits (or a whole number).
fn bytes_of(value: &toml::Value) -> Option<u64> {
    match value {
        toml::Value::String(text) => text.trim().parse().ok(),
        toml::Value::Integer(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

/// Tells the pane what changed: `Changed(<key>, <word>)`. The pane reads the schema again on any
/// `Changed`, so a new or removed account shows by the news of its state row.
async fn announce(served: &OnceLock<Served>, key: &Key, word: &str) {
    if let Some(served) = served.get() {
        let _ = served
            .changed(&KeyPath(path(key)), &toml::Value::String(word.to_owned()))
            .await;
    }
}

impl<H: Hosted, D: Directory, C: Callers> LiveModule for SyncSettings<H, D, C> {
    async fn permit(&self, caller: &Caller, _access: Access) -> Verdict {
        match self.callers.caller_of(&caller.sender.0).await {
            Some(who) if who.role == CallerRole::Settings => Verdict::Allow,
            _ => Verdict::Refuse,
        }
    }

    async fn describe(&self) -> LiveSchema {
        let not_on = self
            .not_on
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        schema(&self.keep.registry(), &self.names, |account| FilesRow {
            offered: self.keep.offers_files(account),
            not_on: not_on.contains(&account.id),
        })
    }

    async fn get(&self, key: &KeyPath) -> Result<toml::Value, LiveError> {
        let registry = self.keep.registry();
        let unknown = || LiveError::UnknownKey(key.0.clone());
        let account = |id: &AccountId| account_of(&registry, id).ok_or_else(unknown);
        let text = |word: &str| Ok(toml::Value::String(word.to_owned()));
        match parse(&key.0, &registry).ok_or_else(unknown)? {
            Key::Label(id) => text(&account(&id)?.label.0),
            Key::Provider(id) => text(account(&id)?.provider.as_str()),
            Key::State(id) => text(state_slug(account(&id)?.state)),
            Key::Sync(id, class) => {
                account(&id)?;
                text(if sync_allowed(&registry.grants, &id, class) {
                    "on"
                } else {
                    "off"
                })
            }
            Key::Files(id) => {
                account(&id)?;
                text(if drive_allowed(&registry.grants, &id) {
                    "on"
                } else {
                    "off"
                })
            }
            Key::Add | Key::Reauth(_) | Key::Remove(_) => Ok(off()),
            Key::CacheLimit => {
                let limit = self
                    .cache
                    .as_ref()
                    .map_or(CacheLimit::Automatic, |(knob, _)| *knob.borrow());
                text(&limit.setting().to_string())
            }
        }
    }

    async fn set(&self, key: &KeyPath, value: toml::Value) -> Result<(), LiveError> {
        let registry = self.keep.registry();
        let parsed =
            parse(&key.0, &registry).ok_or_else(|| LiveError::UnknownKey(key.0.clone()))?;
        match parsed {
            Key::Add => {
                let (keep, served) = (Arc::clone(&self.keep), Arc::clone(&self.served));
                tokio::spawn(async move {
                    let before = keep.registry().accounts.len();
                    let outcome = keep
                        .continue_and_keep(&ParentWindow::Unparented)
                        .await
                        .map(drop);
                    if let Err(why) = &outcome {
                        eprintln!("syncd: continue as: {why}");
                    }
                    announce(&served, &Key::Add, word_of(&outcome)).await;
                    // A new account appears with its state, which a pane reads the schema for.
                    if keep.registry().accounts.len() != before
                        && let Some(account) = keep.registry().accounts.last()
                    {
                        announce(&served, &Key::State(account.id.clone()), "ok").await;
                    }
                });
            }
            Key::Sync(id, class) => {
                let toggle = toggle_of(&value)
                    .ok_or_else(|| LiveError::BadValue("a sync row is `on` or `off`".into()))?;
                self.keep
                    .set_sync(&id, class, toggle)
                    .await
                    .map_err(|why| {
                        eprintln!("syncd: a Settings change was refused: {why}");
                        failed("That could not be done.")
                    })?;
            }
            Key::Files(id) => {
                let toggle = toggle_of(&value)
                    .ok_or_else(|| LiveError::BadValue("this row is `on` or `off`".into()))?;
                self.set_files(id, toggle).await?;
            }
            Key::Reauth(id) => {
                let (keep, served) = (Arc::clone(&self.keep), Arc::clone(&self.served));
                tokio::spawn(async move {
                    let outcome = keep.sign_in_again(&id, ParentWindow::Unparented).await;
                    if let Err(why) = &outcome {
                        eprintln!("syncd: sign in again: {why}");
                    }
                    announce(&served, &Key::Reauth(id), word_of(&outcome)).await;
                });
            }
            Key::Remove(id) => {
                self.keep.remove(&id).await.map_err(failed)?;
                announce(&self.served, &Key::State(id), "removed").await;
            }
            Key::CacheLimit => {
                let bytes = bytes_of(&value).ok_or_else(|| {
                    LiveError::BadValue("the limit is a number of bytes, 0 for automatic".into())
                })?;
                let (knob, file) = self.cache.as_ref().ok_or_else(|| {
                    LiveError::NotPermitted("this computer shows no drives".into())
                })?;
                let limit = CacheLimit::from_setting(bytes);
                write_cache_limit(file, limit).map_err(|why| {
                    eprintln!("syncd: the cache limit could not be saved: {why}");
                    failed("That could not be saved.")
                })?;
                knob.send_replace(limit);
                announce(&self.served, &Key::CacheLimit, &bytes.to_string()).await;
            }
            Key::Label(_) | Key::Provider(_) | Key::State(_) => {
                return Err(LiveError::NotPermitted("this row is a read-out".into()));
            }
        }
        Ok(())
    }
}

impl<H: Hosted, D: Directory, C> SyncSettings<H, D, C> {
    /// "Show in Files". Off answers when the grant is taken back. On may open the provider's
    /// page and wait on the person, longer than a Settings call waits, so it answers at once and
    /// tells the row afterwards: `on`, or `off` with [`FILES_NOT_ON`] under it.
    async fn set_files(&self, id: AccountId, toggle: Toggle) -> Result<(), LiveError> {
        let forget = |not_on: &Mutex<BTreeSet<AccountId>>, id: &AccountId| {
            not_on
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
        };
        forget(&self.not_on, &id);
        if toggle != Toggle::On {
            self.keep.show_in_files(&id, toggle).await.map_err(|why| {
                eprintln!("syncd: a Settings change was refused: {why}");
                failed("That could not be done.")
            })?;
            return Ok(());
        }
        let row = account_of(&self.keep.registry(), &id)
            .ok_or_else(|| LiveError::UnknownKey(path(&Key::Files(id.clone()))))?;
        if !self.keep.offers_files(&row) {
            return Err(LiveError::NotPermitted(
                "this account can't be shown in Files".into(),
            ));
        }
        let (keep, served, not_on) = (
            Arc::clone(&self.keep),
            Arc::clone(&self.served),
            Arc::clone(&self.not_on),
        );
        tokio::spawn(async move {
            let word = match keep.show_in_files(&id, Toggle::On).await {
                Ok(()) => "on",
                Err(why) => {
                    eprintln!("syncd: Show in Files was not turned on: {why}");
                    not_on
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id.clone());
                    "off"
                }
            };
            announce(&served, &Key::Files(id), word).await;
        });
        Ok(())
    }
}

/// Serves the module at [`SETTINGS_PATH`].
pub async fn serve_settings<H: Hosted, D: Directory, C: Callers>(
    connection: &Connection,
    module: SyncSettings<H, D, C>,
) -> zbus::Result<()> {
    let served_slot = Arc::clone(&module.served);
    let served = serve(connection, SETTINGS_PATH, module)
        .await
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    // Served once; a second call leaves the first in place.
    let _ = served_slot.set(served);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_parses_back_from_its_path_and_the_add_row_is_not_an_account() {
        let mut registry = Registry::default();
        let mut account = porter_fake::storage_account();
        account.id = AccountId::parse("graph.acct").expect("id");
        registry.accounts.push(account.clone());
        let id = account.id;
        let keys = [
            Key::Add,
            Key::Label(id.clone()),
            Key::Provider(id.clone()),
            Key::State(id.clone()),
            Key::Sync(id.clone(), SyncClass::Files),
            Key::Sync(id.clone(), SyncClass::Photos),
            Key::Files(id.clone()),
            Key::Reauth(id.clone()),
            Key::Remove(id),
            Key::CacheLimit,
        ];
        for key in keys {
            assert_eq!(parse(&path(&key), &registry), Some(key.clone()), "{key:?}");
        }
        assert_eq!(parse("accounts.other.label", &registry), None);
        assert_eq!(parse("accounts.graph.acct.sync.video", &registry), None);
    }

    #[test]
    fn settings_kept_before_their_folder_are_read_and_the_next_save_moves_them() {
        let dir = porter_fake::Scratch::new("syncd-settings-move");
        let file = dir.join("porter/syncd/syncd.toml");
        let before = dir.join("porter/syncd.toml");
        assert_eq!(read_cache_limit(&file), CacheLimit::Automatic);
        std::fs::create_dir_all(dir.join("porter")).expect("dir");
        std::fs::write(
            &before,
            "[storage]\ncache_limit = \"5000000000\"\n[other]\nkept = true\n",
        )
        .expect("before");
        assert_eq!(
            read_cache_limit(&file),
            CacheLimit::from_setting(5_000_000_000)
        );
        write_cache_limit(&file, CacheLimit::from_setting(3_000_000_000)).expect("save");
        let saved = std::fs::read_to_string(&file).expect("saved at its own place");
        assert!(
            saved.contains("3000000000") && saved.contains("kept = true"),
            "{saved}"
        );
        // The earlier file is left as it was, and no longer read.
        assert!(
            std::fs::read_to_string(&before)
                .expect("left")
                .contains("5000000000")
        );
        assert_eq!(
            read_cache_limit(&file),
            CacheLimit::from_setting(3_000_000_000)
        );
    }
}

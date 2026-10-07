//! A fake Secret Service (`org.freedesktop.secrets`): the freedesktop Secret Service API as far
//! as oo7's D-Bus client uses it, in memory, with the plain algorithm only. It lets the real
//! accountd (`Oo7Secrets`) run in a jail that has no keyring daemon: nothing is encrypted and
//! nothing is kept after the process ends, which is the point.
//!
//! - `OpenSession("plain", "")` opens a session. Any other algorithm is
//!   `org.freedesktop.DBus.Error.NotSupported`, which makes oo7 fall back to plain after its
//!   first, encrypted attempt (`oo7::dbus::Service::new`).
//! - One collection, `default` (the alias `default` names it), never locked; `CreateCollection`
//!   makes more. No prompts: every prompt path is `/`.
//! - Items carry a label, attributes, a secret and its content type; `SearchItems` matches every
//!   attribute asked for; `CreateItem` with `replace` overwrites the item with the same
//!   attributes.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use zbus::object_server::ObjectServer;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Type, Value};
use zbus::{Connection, fdo, interface};

/// The bus name.
pub const BUS_NAME: &str = "org.freedesktop.secrets";
/// The service's object path.
pub const ROOT: &str = "/org/freedesktop/secrets";
const NONE: &str = "/";
const ITEM_LABEL: &str = "org.freedesktop.Secret.Item.Label";
const ITEM_ATTRIBUTES: &str = "org.freedesktop.Secret.Item.Attributes";
const COLLECTION_LABEL: &str = "org.freedesktop.Secret.Collection.Label";

/// A secret as it crosses the bus (`(oayays)`): the session, algorithm parameters (none for
/// plain), the value and its content type.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Secret(pub OwnedObjectPath, pub Vec<u8>, pub Vec<u8>, pub String);

#[derive(Debug, Clone)]
struct ItemRow {
    label: String,
    attributes: BTreeMap<String, String>,
    value: Vec<u8>,
    content_type: String,
    created: u64,
    modified: u64,
}

#[derive(Debug, Default)]
struct CollectionRow {
    label: String,
    items: BTreeMap<u64, ItemRow>,
    created: u64,
}

#[derive(Debug, Default)]
struct Store {
    next: u64,
    sessions: Vec<String>,
    collections: BTreeMap<String, CollectionRow>,
    aliases: BTreeMap<String, String>,
}

/// The state every object shares.
#[derive(Debug, Clone, Default)]
pub struct Shared(Arc<Mutex<Store>>);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn path(text: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(text.to_owned()).unwrap_or_default()
}

fn collection_path(name: &str) -> String {
    format!("{ROOT}/collection/{name}")
}

fn item_path(collection: &str, id: u64) -> String {
    format!("{}/i{id}", collection_path(collection))
}

fn session_path(id: u64) -> String {
    format!("{ROOT}/session/s{id}")
}

/// What went wrong, as the Secret Service API names it.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.Secret.Error")]
pub enum SecretError {
    /// A bus-level failure.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The session is not open.
    NoSession(String),
    /// No such collection or item.
    NoSuchObject(String),
}

impl Shared {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn matching(&self, collection: &str, wanted: &HashMap<String, String>) -> Vec<OwnedObjectPath> {
        let store = self.store();
        let Some(row) = store.collections.get(collection) else {
            return Vec::new();
        };
        row.items
            .iter()
            .filter(|(_, item)| {
                wanted
                    .iter()
                    .all(|(k, v)| item.attributes.get(k).is_some_and(|have| have == v))
            })
            .map(|(id, _)| path(&item_path(collection, *id)))
            .collect()
    }

    fn open_session(&self) -> String {
        let mut store = self.store();
        store.next += 1;
        let session = session_path(store.next);
        store.sessions.push(session.clone());
        session
    }

    fn session_is_open(&self, session: &str) -> bool {
        self.store().sessions.iter().any(|s| s == session)
    }

    fn make_collection(&self, label: &str, alias: &str) -> String {
        let mut store = self.store();
        let name = match alias.is_empty() {
            false if !store.collections.contains_key(alias) => alias.to_owned(),
            _ => {
                store.next += 1;
                format!("c{}", store.next)
            }
        };
        store
            .collections
            .entry(name.clone())
            .or_insert_with(|| CollectionRow {
                label: label.to_owned(),
                created: now(),
                ..CollectionRow::default()
            });
        if !alias.is_empty() {
            store.aliases.insert(alias.to_owned(), name.clone());
        }
        name
    }
}

/// `org.freedesktop.Secret.Service` at [`ROOT`].
#[derive(Debug)]
pub struct Service {
    shared: Shared,
}

fn wanted_of(properties: &HashMap<String, OwnedValue>) -> (String, BTreeMap<String, String>) {
    let label = properties
        .get(ITEM_LABEL)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default();
    let attributes = properties
        .get(ITEM_ATTRIBUTES)
        .and_then(|v| HashMap::<String, String>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    (label, attributes)
}

#[interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    async fn open_session(
        &self,
        algorithm: &str,
        input: Value<'_>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<(OwnedValue, OwnedObjectPath)> {
        let _ = input;
        if algorithm != "plain" {
            return Err(fdo::Error::NotSupported(format!(
                "only the plain algorithm is served, not {algorithm}"
            )));
        }
        let session = self.shared.open_session();
        let object = Session {
            shared: self.shared.clone(),
            path: session.clone(),
        };
        server.at(path(&session), object).await?;
        let empty = Value::from("")
            .try_to_owned()
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok((empty, path(&session)))
    }

    async fn create_collection(
        &self,
        properties: HashMap<String, OwnedValue>,
        alias: &str,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        let label = properties
            .get(COLLECTION_LABEL)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default();
        let existed = alias_target(&self.shared, alias);
        let name = existed.unwrap_or_else(|| self.shared.make_collection(&label, alias));
        register_collection(&self.shared, server, &name).await?;
        Ok((path(&collection_path(&name)), path(NONE)))
    }

    fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let names: Vec<String> = self.shared.store().collections.keys().cloned().collect();
        let found = names
            .iter()
            .flat_map(|name| self.shared.matching(name, &attributes))
            .collect();
        (found, Vec::new())
    }

    fn unlock(&self, objects: Vec<ObjectPath<'_>>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        let all = objects.iter().map(|o| path(o.as_str())).collect();
        (all, path(NONE))
    }

    fn lock(&self, objects: Vec<ObjectPath<'_>>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        // Nothing here ever locks: nothing was locked.
        let _ = objects;
        (Vec::new(), path(NONE))
    }

    fn get_secrets(
        &self,
        items: Vec<ObjectPath<'_>>,
        session: ObjectPath<'_>,
    ) -> Result<HashMap<OwnedObjectPath, Secret>, SecretError> {
        if !self.shared.session_is_open(session.as_str()) {
            return Err(SecretError::NoSession(session.to_string()));
        }
        let mut out = HashMap::new();
        for item in items {
            if let Some(row) = find_item(&self.shared, item.as_str()) {
                out.insert(
                    path(item.as_str()),
                    Secret(
                        path(session.as_str()),
                        Vec::new(),
                        row.value,
                        row.content_type,
                    ),
                );
            }
        }
        Ok(out)
    }

    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        alias_target(&self.shared, name).map_or_else(|| path(NONE), |c| path(&collection_path(&c)))
    }

    fn set_alias(&self, name: &str, collection: ObjectPath<'_>) {
        let prefix = format!("{ROOT}/collection/");
        let Some(target) = collection.as_str().strip_prefix(&prefix) else {
            return;
        };
        self.shared
            .store()
            .aliases
            .insert(name.to_owned(), target.to_owned());
    }

    #[zbus(property)]
    fn collections(&self) -> Vec<OwnedObjectPath> {
        self.shared
            .store()
            .collections
            .keys()
            .map(|name| path(&collection_path(name)))
            .collect()
    }
}

fn alias_target(shared: &Shared, alias: &str) -> Option<String> {
    let store = shared.store();
    store.aliases.get(alias).cloned()
}

/// `<root>/collection/<name>/i<id>` as (collection name, id).
fn split_item(text: &str) -> Option<(String, u64)> {
    let rest = text.strip_prefix(&format!("{ROOT}/collection/"))?;
    let (collection, item) = rest.split_once('/')?;
    Some((collection.to_owned(), item.strip_prefix('i')?.parse().ok()?))
}

fn find_item(shared: &Shared, text: &str) -> Option<ItemRow> {
    let (collection, id) = split_item(text)?;
    shared
        .store()
        .collections
        .get(&collection)?
        .items
        .get(&id)
        .cloned()
}

/// A session: closing it forgets it.
#[derive(Debug)]
pub struct Session {
    shared: Shared,
    path: String,
}

#[interface(name = "org.freedesktop.Secret.Session")]
impl Session {
    async fn close(&self, #[zbus(object_server)] server: &ObjectServer) -> fdo::Result<()> {
        self.shared.store().sessions.retain(|s| *s != self.path);
        server.remove::<Session, _>(path(&self.path)).await?;
        Ok(())
    }
}

/// `org.freedesktop.Secret.Collection`.
#[derive(Debug)]
pub struct Collection {
    shared: Shared,
    name: String,
}

#[interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
    async fn delete(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        let removed = {
            let mut store = self.shared.store();
            store.aliases.retain(|_, target| *target != self.name);
            store.collections.remove(&self.name)
        };
        if let Some(row) = removed {
            for id in row.items.keys() {
                server
                    .remove::<Item, _>(path(&item_path(&self.name, *id)))
                    .await?;
            }
        }
        server
            .remove::<Collection, _>(path(&collection_path(&self.name)))
            .await?;
        Ok(path(NONE))
    }

    fn search_items(&self, attributes: HashMap<String, String>) -> Vec<OwnedObjectPath> {
        self.shared.matching(&self.name, &attributes)
    }

    async fn create_item(
        &self,
        properties: HashMap<String, OwnedValue>,
        secret: Secret,
        replace: bool,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath), SecretError> {
        let (label, attributes) = wanted_of(&properties);
        let Secret(session, _parameters, value, content_type) = secret;
        if !self.shared.session_is_open(session.as_str()) {
            return Err(SecretError::NoSession(session.to_string()));
        }
        let wanted: HashMap<String, String> = attributes.clone().into_iter().collect();
        let existing = match replace {
            true => self.shared.matching(&self.name, &wanted).into_iter().next(),
            false => None,
        };
        let at = now();
        let created = {
            let mut store = self.shared.store();
            store.next += 1;
            let id = store.next;
            let row = store
                .collections
                .get_mut(&self.name)
                .ok_or_else(|| SecretError::NoSuchObject(self.name.clone()))?;
            match &existing {
                Some(old) => {
                    let (_, old_id) = split_item(old.as_str()).unwrap_or_default();
                    if let Some(item) = row.items.get_mut(&old_id) {
                        item.label = label;
                        item.value = value;
                        item.content_type = content_type;
                        item.modified = at;
                    }
                    None
                }
                None => {
                    row.items.insert(
                        id,
                        ItemRow {
                            label,
                            attributes,
                            value,
                            content_type,
                            created: at,
                            modified: at,
                        },
                    );
                    Some(id)
                }
            }
        };
        let item = match (existing, created) {
            (Some(old), _) => old,
            (None, Some(id)) => {
                let text = item_path(&self.name, id);
                let object = Item {
                    shared: self.shared.clone(),
                    collection: self.name.clone(),
                    id,
                };
                server.at(path(&text), object).await?;
                path(&text)
            }
            (None, None) => path(NONE),
        };
        Ok((item, path(NONE)))
    }

    #[zbus(property)]
    fn items(&self) -> Vec<OwnedObjectPath> {
        self.shared.matching(&self.name, &HashMap::new())
    }

    #[zbus(property)]
    fn label(&self) -> String {
        self.shared
            .store()
            .collections
            .get(&self.name)
            .map(|c| c.label.clone())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn set_label(&mut self, label: String) {
        if let Some(row) = self.shared.store().collections.get_mut(&self.name) {
            row.label = label;
        }
    }

    #[zbus(property)]
    fn locked(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn created(&self) -> u64 {
        self.shared
            .store()
            .collections
            .get(&self.name)
            .map_or(0, |c| c.created)
    }

    #[zbus(property)]
    fn modified(&self) -> u64 {
        now()
    }
}

/// `org.freedesktop.Secret.Item`.
#[derive(Debug)]
pub struct Item {
    shared: Shared,
    collection: String,
    id: u64,
}

impl Item {
    fn row<T>(&self, read: impl FnOnce(&ItemRow) -> T) -> Option<T> {
        let store = self.shared.store();
        store
            .collections
            .get(&self.collection)?
            .items
            .get(&self.id)
            .map(read)
    }

    fn edit(&self, change: impl FnOnce(&mut ItemRow)) {
        if let Some(row) = self
            .shared
            .store()
            .collections
            .get_mut(&self.collection)
            .and_then(|c| c.items.get_mut(&self.id))
        {
            change(row);
            row.modified = now();
        }
    }
}

#[interface(name = "org.freedesktop.Secret.Item")]
impl Item {
    async fn delete(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<OwnedObjectPath> {
        if let Some(row) = self.shared.store().collections.get_mut(&self.collection) {
            row.items.remove(&self.id);
        }
        server
            .remove::<Item, _>(path(&item_path(&self.collection, self.id)))
            .await?;
        Ok(path(NONE))
    }

    fn get_secret(&self, session: ObjectPath<'_>) -> Result<Secret, SecretError> {
        if !self.shared.session_is_open(session.as_str()) {
            return Err(SecretError::NoSession(session.to_string()));
        }
        self.row(|row| {
            Secret(
                path(session.as_str()),
                Vec::new(),
                row.value.clone(),
                row.content_type.clone(),
            )
        })
        .ok_or_else(|| SecretError::NoSuchObject(item_path(&self.collection, self.id)))
    }

    fn set_secret(&self, secret: Secret) -> Result<(), SecretError> {
        let Secret(session, _parameters, value, content_type) = secret;
        if !self.shared.session_is_open(session.as_str()) {
            return Err(SecretError::NoSession(session.to_string()));
        }
        self.edit(|row| {
            row.value = value;
            row.content_type = content_type;
        });
        Ok(())
    }

    #[zbus(property)]
    fn locked(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn attributes(&self) -> HashMap<String, String> {
        self.row(|row| row.attributes.clone().into_iter().collect())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn set_attributes(&mut self, attributes: HashMap<String, String>) {
        self.edit(|row| row.attributes = attributes.into_iter().collect());
    }

    #[zbus(property)]
    fn label(&self) -> String {
        self.row(|row| row.label.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    fn set_label(&mut self, label: String) {
        self.edit(|row| row.label = label);
    }

    #[zbus(property)]
    fn created(&self) -> u64 {
        self.row(|row| row.created).unwrap_or_default()
    }

    #[zbus(property)]
    fn modified(&self) -> u64 {
        self.row(|row| row.modified).unwrap_or_default()
    }
}

async fn register_collection(
    shared: &Shared,
    server: &ObjectServer,
    name: &str,
) -> zbus::Result<()> {
    let object = Collection {
        shared: shared.clone(),
        name: name.to_owned(),
    };
    // Registering a collection twice is not an error: the first object stays.
    server.at(path(&collection_path(name)), object).await?;
    Ok(())
}

/// Serves the Secret Service on `connection` (which must be on the bus the clients use) and takes
/// the name [`BUS_NAME`]: one empty collection `default`, aliased `default`.
pub async fn serve(connection: &Connection) -> zbus::Result<Shared> {
    let shared = Shared::default();
    shared.make_collection("Default", "default");
    let server = connection.object_server();
    register_collection(&shared, server, "default").await?;
    server
        .at(
            path(ROOT),
            Service {
                shared: shared.clone(),
            },
        )
        .await?;
    connection.request_name(BUS_NAME).await?;
    Ok(shared)
}

//! Computers the person adds in Settings (`Inference1.AddComputer` and `RemoveComputer`).
//!
//! inferd never rewrites the person's hand-written `inferd.toml`. What Settings adds lives in
//! inferd's own file, `computers.toml` in its state directory (the one place the unit may write),
//! and is merged with the hand-written engines when the daemon starts, so a computer added here
//! is the same kind of attached engine as one written by hand: `where = "my-network"`, named by
//! its `computer`, with every check [`AttachedEntry::check`] gives the file.
//!
//! A computer is a name and its models. Each model says how it is reached (a path on this
//! computer, or a port on this computer: the person's tunnel ends there) and may have a key. A
//! key arrives as text, is written to a file of its own in the same state directory with the
//! owner as its only reader (`AtomicWrite::PRIVATE`), and is never given back: no reply, log line
//! or `Debug` output holds it. The file of the computers holds the path of the key file, not the
//! key.
//!
//! A computer on the person's Tailscale network (`Inference1.AddTailnetComputer`) is kept in the
//! same file, with its node id and the models it lends, and no address and no key: it is reached
//! through a relay at a socket of inferd's own ([`relay_socket`]), which asks Tailscale who is at
//! the address on every connection.
//!
//! The words of every refusal are plain: the person reads them in Settings.

use super::book::AttachedBook;
use super::config::{Attached, AttachedEntry, AttachedError, Place, Reach};
use super::model::local_model;
use crate::startup::SUN_PATH;
use porter_core::{ModelId, NodeId, SecretText};
use porter_fs::atomic::AtomicWrite;
use porter_infer::{ComputerName, PlaceId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The most models one computer may have here.
pub const MOST_MODELS: usize = 32;

/// The longest name a person gives a computer, in characters.
const LONGEST_LABEL: usize = 64;

/// The longest key kept, in bytes (as the key file check: a token is a few dozen).
const LONGEST_KEY: usize = 4096;

/// One model of an added computer, as the file holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddedModel {
    /// The path of the connection on this computer that leads to the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<PathBuf>,
    /// The port on this computer that leads to the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The file inferd keeps the model's key in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
}

impl AddedModel {
    /// The attached-engine entry this is, for a computer called `computer`.
    pub fn entry(&self, computer: &str) -> AttachedEntry {
        AttachedEntry {
            socket: self.socket.clone(),
            url: self.port.map(|port| format!("http://127.0.0.1:{port}")),
            key_file: self.key_file.clone(),
            place: Some(Place::MyNetwork),
            computer: Some(computer.to_owned()),
        }
    }
}

/// One added computer, as the file holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddedComputer {
    /// The name the person gave it.
    pub label: String,
    /// Its Tailscale node id, for a computer on the person's Tailscale network (its models then
    /// name no socket, port or key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Its models, by catalogue id.
    #[serde(default)]
    pub models: BTreeMap<String, AddedModel>,
}

/// The socket of the relay that leads to the computer `node`, in `sockets`: the one place a
/// computer on the network is reached from. None when the path would be too long to connect to.
pub fn relay_socket(sockets: &Path, node: &NodeId) -> Option<PathBuf> {
    let path = sockets.join(format!("tailnet-{node}.sock"));
    (path.as_os_str().as_bytes().len() < SUN_PATH).then_some(path)
}

/// `computers.toml`: the computers Settings added, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddedFile {
    /// The computers, by the name their place id carries.
    #[serde(default)]
    pub computers: BTreeMap<String, AddedComputer>,
}

/// Why the file of added computers was not used as it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddedProblem {
    /// It could not be read.
    #[error("{}: cannot be read ({kind:?})", path.display())]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What the system said.
        kind: std::io::ErrorKind,
    },
    /// It is not a list of computers; it was put aside and the list starts empty.
    #[error("{}: is not a list of computers; kept as {} and started empty", path.display(), aside.display())]
    Unusable {
        /// The file.
        path: PathBuf,
        /// Where it was put.
        aside: PathBuf,
    },
}

impl AddedFile {
    /// The file at `path`; a missing one is the empty list. One that is not a list of computers is
    /// moved aside (`computers.toml.unusable`) and the list starts empty, so a later change does
    /// not write over what the person may want back.
    pub fn read(path: &Path) -> Result<Self, AddedProblem> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(AddedProblem::Unreadable {
                    path: path.to_owned(),
                    kind: e.kind(),
                });
            }
        };
        match toml::from_str(&text) {
            Ok(file) => Ok(file),
            Err(_) => {
                let mut aside = path.as_os_str().to_owned();
                aside.push(".unusable");
                let aside = PathBuf::from(aside);
                let _ = std::fs::rename(path, &aside);
                Err(AddedProblem::Unusable {
                    path: path.to_owned(),
                    aside,
                })
            }
        }
    }

    /// The attached engines the file names, each past the checks the file alone allows, and a
    /// line for each that is refused (never the key).
    pub fn attached(&self, sockets: &Path) -> (Vec<Attached>, Vec<String>) {
        let mut engines = Vec::new();
        let mut refused = Vec::new();
        for (name, computer) in &self.computers {
            let Ok(computer_name) = ComputerName::parse(name) else {
                refused.push(format!("computers.{name}: not a computer name"));
                continue;
            };
            if let Some(node) = &computer.node {
                match tailnet_engines(&computer_name, node, computer, sockets) {
                    Ok(found) => engines.extend(found),
                    Err(why) => refused.push(format!("computers.{name}: {why}")),
                }
                continue;
            }
            for (model, added) in &computer.models {
                match added.entry(name).check(model) {
                    Ok(engine) => engines.push(engine),
                    Err(why) => refused.push(why.to_string()),
                }
            }
        }
        (engines, refused)
    }

    /// The names the person gave, by computer name.
    pub fn labels(&self) -> BTreeMap<ComputerName, String> {
        self.computers
            .iter()
            .filter_map(|(name, computer)| {
                Some((ComputerName::parse(name).ok()?, computer.label.clone()))
            })
            .collect()
    }
}

/// The engines of a computer on the Tailscale network: one per model it lends, each reached
/// through the relay at [`relay_socket`].
fn tailnet_engines(
    name: &ComputerName,
    node: &str,
    computer: &AddedComputer,
    sockets: &Path,
) -> Result<Vec<Attached>, &'static str> {
    let node = NodeId::parse(node).map_err(|_| "not a computer on Tailscale")?;
    let socket = relay_socket(sockets, &node).ok_or("the way to it is too long")?;
    Ok(computer
        .models
        .keys()
        .filter_map(|model| ModelId::parse(model).ok())
        .map(|id| Attached {
            id,
            reach: Reach::Tailnet {
                node: node.clone(),
                socket: socket.clone(),
            },
            key_file: None,
            place: Place::MyNetwork,
            computer: Some(name.clone()),
        })
        .collect())
}

/// The computers of the hand-written file: the names their engines carry (`other-computer` for an
/// engine on another machine that names none).
pub fn hand_written(engines: &[Attached]) -> BTreeSet<ComputerName> {
    engines
        .iter()
        .filter(|engine| engine.place == Place::MyNetwork)
        .map(|engine| engine.computer.clone().unwrap_or_else(ComputerName::other))
        .collect()
}

/// What the file of added computers contributes next to the hand-written engines: its engines
/// without those whose computer or model the hand-written file already has (the hand-written
/// file wins, and each one left out is said).
pub fn merged(
    hand: &[Attached],
    added: &AddedFile,
    sockets: &Path,
) -> (Vec<Attached>, Vec<String>) {
    let (engines, mut said) = added.attached(sockets);
    let names = hand_written(hand);
    let models: BTreeSet<&ModelId> = hand.iter().map(|engine| &engine.id).collect();
    let kept = engines
        .into_iter()
        .filter(|engine| {
            let clash_name = engine
                .computer
                .as_ref()
                .is_some_and(|computer| names.contains(computer));
            let clash_model = models.contains(&engine.id);
            if clash_name || clash_model {
                said.push(format!(
                    "computers: {} is also in the settings file; the settings file wins",
                    engine.id
                ));
            }
            !(clash_name || clash_model)
        })
        .collect();
    (kept, said)
}

/// How a new model is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewReach {
    /// By the connection at this path on this computer.
    Socket(PathBuf),
    /// By this port on this computer.
    Port(u16),
}

/// One model of a computer being added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewModel {
    /// The model's catalogue id.
    pub id: String,
    /// How it is reached.
    pub reach: NewReach,
    /// Its key, if it wants one.
    pub key: Option<SecretText>,
}

/// A computer being added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComputer {
    /// The name the person gave it.
    pub label: String,
    /// Its models.
    pub models: Vec<NewModel>,
}

/// A computer on the person's Tailscale network being added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTailnetComputer {
    /// The name people call it by, which the place's name is made from.
    pub label: String,
    /// Its stable Tailscale id.
    pub node: NodeId,
    /// The catalogue ids of the models it lends.
    pub models: Vec<String>,
}

/// Why a computer was not added or removed. `Display` is the plain sentence Settings shows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComputerError {
    /// The name is empty, too long or has control characters.
    #[error("Give the computer a name of one short line.")]
    BadName,
    /// No model was given.
    #[error("Add at least one model for this computer.")]
    NoModels,
    /// More models than one computer may have here.
    #[error("That is more models than one computer can have here.")]
    TooMany,
    /// A computer of that name is already there, added here or written by hand.
    #[error("A computer with that name is already there.")]
    AlreadyThere,
    /// The assistant does not know the model.
    #[error("The assistant does not know a model called \u{201c}{0}\u{201d}.")]
    UnknownModel(String),
    /// The model is one that cannot be used from another computer.
    #[error("The model \u{201c}{0}\u{201d} cannot be used from another computer.")]
    ModelNotUsable(String),
    /// The model is already set up, on this computer or another.
    #[error("The model \u{201c}{0}\u{201d} is already set up on a computer.")]
    ModelTaken(String),
    /// How to reach the model is not something that can be used.
    #[error(
        "\u{201c}{0}\u{201d} cannot be reached that way. Give the full path of a connection on this computer, or a port on this computer."
    )]
    BadAddress(String),
    /// The key is empty, too long, or has more than one line.
    #[error("The key for \u{201c}{0}\u{201d} cannot be used.")]
    BadKey(String),
    /// The new list could not be saved.
    #[error("The computer could not be saved.")]
    NotSaved,
    /// There is no computer of that name.
    #[error("There is no computer with that name.")]
    NotThere,
    /// The computer is written in the settings file by hand.
    #[error(
        "That computer was added by hand in the settings file, so it can only be removed there."
    )]
    AddedByHand,
    /// Adding computers is not set up in this daemon.
    #[error("Adding computers is not available here.")]
    Unavailable,
    /// The computer is not one of the person's own on their Tailscale network.
    #[error("That is not one of your computers on Tailscale.")]
    NotOnTailscale,
    /// The computer does not lend its models right now.
    #[error(
        "That computer isn't lending its models right now. Check that it is on, and that it is set to let your other computers use its models."
    )]
    NotAnswering,
    /// The computer is not asking to use this computer's models.
    #[error("That computer is not asking to use this computer's models.")]
    NotAsking,
    /// Too many computers are asking at once.
    #[error("Too many computers are asking at once. Answer some of them first.")]
    TooManyAsking,
}

impl From<porter_tailnet::GuestError> for ComputerError {
    fn from(error: porter_tailnet::GuestError) -> Self {
        use porter_tailnet::GuestError;
        match error {
            GuestError::NotAsking => ComputerError::NotAsking,
            GuestError::TooManyAsking => ComputerError::TooManyAsking,
            GuestError::TooMany => ComputerError::TooMany,
            _ => ComputerError::NotSaved,
        }
    }
}

impl ComputerError {
    /// The name of the error on the bus, after `org.quire.Inference1.Error.Computer.`.
    pub fn name(&self) -> &'static str {
        match self {
            ComputerError::BadName => "BadName",
            ComputerError::NoModels => "NoModels",
            ComputerError::TooMany => "TooMany",
            ComputerError::AlreadyThere => "AlreadyThere",
            ComputerError::UnknownModel(_) => "UnknownModel",
            ComputerError::ModelNotUsable(_) => "ModelNotUsable",
            ComputerError::ModelTaken(_) => "ModelTaken",
            ComputerError::BadAddress(_) => "BadAddress",
            ComputerError::BadKey(_) => "BadKey",
            ComputerError::NotSaved => "NotSaved",
            ComputerError::NotThere => "NotThere",
            ComputerError::AddedByHand => "AddedByHand",
            ComputerError::Unavailable => "Unavailable",
            ComputerError::NotOnTailscale => "NotOnTailscale",
            ComputerError::NotAnswering => "NotAnswering",
            ComputerError::NotAsking => "NotAsking",
            ComputerError::TooManyAsking => "TooManyAsking",
        }
    }
}

/// What the checks of the file say of model `id`, in the words of an addition.
fn refusal_of(why: &AttachedError, id: &str) -> ComputerError {
    let id = id.to_owned();
    match why {
        AttachedError::BadId { .. } | AttachedError::NotInCatalogue { .. } => {
            ComputerError::UnknownModel(id)
        }
        AttachedError::NoCapability { .. }
        | AttachedError::NotAttachable { .. }
        | AttachedError::NoChatEngine { .. } => ComputerError::ModelNotUsable(id),
        AttachedError::RelativeKeyFile { .. } => ComputerError::BadKey(id),
        AttachedError::BadComputer { .. } => ComputerError::BadName,
        AttachedError::MissingWhere { .. }
        | AttachedError::NoTarget { .. }
        | AttachedError::BothTargets { .. }
        | AttachedError::RelativeSocket { .. }
        | AttachedError::SocketPathTooLong { .. }
        | AttachedError::BadUrl { .. }
        | AttachedError::NotPlainHttp { .. }
        | AttachedError::NotLoopback { .. } => ComputerError::BadAddress(id),
    }
}

/// What names a computer whose label has no ASCII letter or digit (a name in Chinese, an emoji).
enum Fallback<'a> {
    /// A computer added by hand: `computer-` and eight hex digits of a SHA-256 of the label's
    /// identity ([`identity_of`]), so the same label gives the same name in every run and
    /// version of Rust.
    Label,
    /// A computer on the Tailscale network: `tailnet-` and its node id in lowercase.
    Node(&'a NodeId),
}

/// The name a place id carries for what the person typed: lowercase letters and digits, other
/// runs of characters one dash, no dash at either end. A label with none of those (a name in
/// another script) is named by `fallback` instead, so no script is refused. The name is never
/// shown (Settings shows the label); it only has to be stable and unique.
fn slug_of(label: &str, fallback: Fallback<'_>) -> ComputerName {
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.truncate(LONGEST_LABEL);
    let slug = slug.trim_end_matches('-');
    let text = if slug.is_empty() {
        match fallback {
            Fallback::Label => {
                let digest = Sha256::digest(identity_of(label).as_bytes());
                let head = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
                format!("computer-{head:08x}")
            }
            Fallback::Node(node) => {
                let mut text = format!("tailnet-{}", node.as_str().to_ascii_lowercase());
                text.truncate(LONGEST_LABEL);
                text
            }
        }
    } else {
        slug.to_owned()
    };
    ComputerName::parse(&text).unwrap_or_else(|_| ComputerName::other())
}

/// What makes two labels the same computer: lowercase, every run of characters that are not
/// letters or digits (of any script) one dash, no dash at either end. A label with no letter or
/// digit at all is its trimmed lowercase self.
fn identity_of(label: &str) -> String {
    let mut key = String::new();
    for c in label.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            key.push(c);
        } else if !key.is_empty() && !key.ends_with('-') {
            key.push('-');
        }
    }
    let key = key.trim_end_matches('-');
    if key.is_empty() {
        label.trim().to_lowercase()
    } else {
        key.to_owned()
    }
}

/// What the person typed for a name, trimmed; none if it is empty, too long or has control
/// characters.
fn clean_label(label: &str) -> Option<String> {
    let label = label.trim();
    let fine = !label.is_empty()
        && label.chars().count() <= LONGEST_LABEL
        && !label.chars().any(char::is_control);
    fine.then(|| label.to_owned())
}

/// Whether `key` can be a key: not empty, not long, one line.
fn usable_key(key: &SecretText) -> bool {
    let text = key.expose();
    !text.trim().is_empty() && text.len() <= LONGEST_KEY && !text.contains(['\n', '\r', '\0'])
}

/// `base` if nothing is `taken` by that name, else `base-2`, `base-3`, ... the first free: the
/// names go in the order the computers were added, and the one chosen is stored, so it never
/// changes.
fn free_name(base: ComputerName, taken: impl Fn(&str) -> bool) -> ComputerName {
    let mut candidate = base.clone();
    let mut n = 1_u32;
    while taken(candidate.as_str()) {
        n += 1;
        let suffix = format!("-{n}");
        let stem = &base.as_str()[..base.as_str().len().min(LONGEST_LABEL - suffix.len())];
        candidate =
            ComputerName::parse(&format!("{stem}{suffix}")).unwrap_or_else(|_| base.clone());
        if candidate == base {
            break;
        }
    }
    candidate
}

#[derive(Debug)]
struct State {
    file: AddedFile,
    by_hand: BTreeSet<ComputerName>,
    hand_models: BTreeSet<ModelId>,
}

impl State {
    /// Whether a computer already has the name `text`, written by hand or added.
    fn is_taken(&self, text: &str) -> bool {
        self.file.computers.contains_key(text)
            || self.by_hand.iter().any(|name| name.as_str() == text)
    }
}

/// The computers Settings adds and removes: the file, the keys beside it and the engines live in
/// the daemon's book.
#[derive(Debug)]
pub struct Computers {
    file: PathBuf,
    keys: PathBuf,
    book: AttachedBook,
    state: Mutex<State>,
}

impl Computers {
    /// The computers kept in `file`, with their keys under `keys`, over the daemon's `book`.
    /// `loaded` is what the file held at start and `hand` the hand-written engines (their
    /// computers and models cannot be added again).
    pub fn new(
        file: PathBuf,
        keys: PathBuf,
        book: AttachedBook,
        loaded: AddedFile,
        hand: &[Attached],
    ) -> Self {
        Self {
            file,
            keys,
            book,
            state: Mutex::new(State {
                file: loaded,
                by_hand: hand_written(hand),
                hand_models: hand.iter().map(|engine| engine.id.clone()).collect(),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key_path(&self, computer: &ComputerName, model: &ModelId) -> PathBuf {
        self.keys.join(format!("{computer}--{model}.key"))
    }

    fn save(&self, file: &AddedFile) -> Result<(), ComputerError> {
        let text = toml::to_string_pretty(file).map_err(|_| ComputerError::NotSaved)?;
        AtomicWrite::PRIVATE
            .write(&self.file, text.as_bytes())
            .map_err(|_| ComputerError::NotSaved)
    }

    /// Adds a computer and its models: the checks of the hand-written file, then the keys, the
    /// file, and last the engines (so a refusal changes nothing). The place it is now.
    pub fn add(&self, new: NewComputer) -> Result<PlaceId, ComputerError> {
        let label = clean_label(&new.label).ok_or(ComputerError::BadName)?;
        let name = slug_of(&label, Fallback::Label);
        if new.models.is_empty() {
            return Err(ComputerError::NoModels);
        }
        if new.models.len() > MOST_MODELS {
            return Err(ComputerError::TooMany);
        }
        let mut state = self.state();
        let identity = identity_of(&label);
        let same_label =
            state.file.computers.values().any(|computer| {
                computer.node.is_none() && identity_of(&computer.label) == identity
            });
        if same_label || state.by_hand.contains(&name) {
            return Err(ComputerError::AlreadyThere);
        }
        let name = free_name(name, |text| state.is_taken(text));
        let mut taken: BTreeSet<String> = state
            .hand_models
            .iter()
            .map(ToString::to_string)
            .chain(
                state
                    .file
                    .computers
                    .values()
                    .flat_map(|computer| computer.models.keys().cloned()),
            )
            .collect();
        let mut added = AddedComputer {
            label: label.clone(),
            node: None,
            models: BTreeMap::new(),
        };
        let mut keys: Vec<(PathBuf, &SecretText)> = Vec::new();
        let mut engines = Vec::new();
        for model in &new.models {
            let id = ModelId::parse(&model.id)
                .map_err(|_| ComputerError::UnknownModel(model.id.clone()))?;
            if !taken.insert(id.to_string()) {
                return Err(ComputerError::ModelTaken(model.id.clone()));
            }
            if model.key.as_ref().is_some_and(|key| !usable_key(key)) {
                return Err(ComputerError::BadKey(model.id.clone()));
            }
            let key_file = model.key.as_ref().map(|_| self.key_path(&name, &id));
            let entry = AddedModel {
                socket: match &model.reach {
                    NewReach::Socket(path) => Some(path.clone()),
                    NewReach::Port(_) => None,
                },
                port: match &model.reach {
                    NewReach::Port(port) => Some(*port),
                    NewReach::Socket(_) => None,
                },
                key_file: key_file.clone(),
            };
            let attached = entry
                .entry(name.as_str())
                .check(id.as_str())
                .map_err(|why| refusal_of(&why, &model.id))?;
            let local = local_model(&attached, self.book.catalogue(), self.book.sockets())
                .map_err(|why| refusal_of(&why, &model.id))?;
            if let (Some(path), Some(key)) = (key_file, &model.key) {
                keys.push((path, key));
            }
            added.models.insert(id.to_string(), entry);
            engines.push(local);
        }
        let mut written: Vec<&Path> = Vec::new();
        for (path, key) in &keys {
            if AtomicWrite::PRIVATE
                .write(path, key.expose().as_bytes())
                .is_err()
            {
                forget(&written);
                return Err(ComputerError::NotSaved);
            }
            written.push(path);
        }
        let mut next = state.file.clone();
        next.computers.insert(name.to_string(), added);
        if let Err(why) = self.save(&next) {
            forget(&written);
            return Err(why);
        }
        state.file = next;
        self.book.add(name.clone(), label, engines);
        Ok(PlaceId::computer(&name))
    }

    /// Adds a computer on the person's Tailscale network and the models it lends. Nothing else
    /// is asked of the caller: no address and no key, since the relay at [`relay_socket`] leads
    /// to it. A refusal changes nothing. The place it is now.
    pub fn add_tailnet(&self, new: NewTailnetComputer) -> Result<PlaceId, ComputerError> {
        let label = clean_label(&new.label).ok_or(ComputerError::BadName)?;
        let name = slug_of(&label, Fallback::Node(&new.node));
        if new.models.is_empty() {
            return Err(ComputerError::NoModels);
        }
        if new.models.len() > MOST_MODELS {
            return Err(ComputerError::TooMany);
        }
        let socket = relay_socket(self.book.sockets(), &new.node)
            .ok_or_else(|| ComputerError::BadAddress(new.node.to_string()))?;
        let mut state = self.state();
        let node_text = new.node.to_string();
        let known_node = state
            .file
            .computers
            .values()
            .any(|computer| computer.node.as_deref() == Some(node_text.as_str()));
        if known_node {
            return Err(ComputerError::AlreadyThere);
        }
        let name = free_name(name, |text| state.is_taken(text));
        let mut taken: BTreeSet<String> = state
            .hand_models
            .iter()
            .map(ToString::to_string)
            .chain(
                state
                    .file
                    .computers
                    .values()
                    .flat_map(|computer| computer.models.keys().cloned()),
            )
            .collect();
        let mut added = AddedComputer {
            label: label.clone(),
            node: Some(node_text),
            models: BTreeMap::new(),
        };
        let mut engines = Vec::new();
        for model in &new.models {
            let id =
                ModelId::parse(model).map_err(|_| ComputerError::UnknownModel(model.clone()))?;
            if !taken.insert(id.to_string()) {
                return Err(ComputerError::ModelTaken(model.clone()));
            }
            let attached = Attached {
                id: id.clone(),
                reach: Reach::Tailnet {
                    node: new.node.clone(),
                    socket: socket.clone(),
                },
                key_file: None,
                place: Place::MyNetwork,
                computer: Some(name.clone()),
            };
            let local = local_model(&attached, self.book.catalogue(), self.book.sockets())
                .map_err(|why| refusal_of(&why, model))?;
            added.models.insert(id.to_string(), AddedModel::default());
            engines.push(local);
        }
        let mut next = state.file.clone();
        next.computers.insert(name.to_string(), added);
        self.save(&next)?;
        state.file = next;
        self.book.add(name.clone(), label, engines);
        Ok(PlaceId::computer(&name))
    }

    /// The computers on the Tailscale network that were added, by node id, with the name their
    /// place id carries.
    pub fn tailnet_nodes(&self) -> BTreeMap<NodeId, ComputerName> {
        self.state()
            .file
            .computers
            .iter()
            .filter_map(|(name, computer)| {
                Some((
                    NodeId::parse(computer.node.as_deref()?).ok()?,
                    ComputerName::parse(name).ok()?,
                ))
            })
            .collect()
    }

    /// Makes the names people read follow the names Tailscale knows the computers by (`names`,
    /// by node id): the place id stays, so what was saved of it keeps working. Whether any name
    /// changed.
    pub fn relabel(&self, names: &BTreeMap<NodeId, String>) -> bool {
        let mut state = self.state();
        let mut next = state.file.clone();
        let mut changed = Vec::new();
        for (name, computer) in &mut next.computers {
            let Some(fresh) = computer
                .node
                .as_deref()
                .and_then(|node| NodeId::parse(node).ok())
                .and_then(|node| names.get(&node))
                .and_then(|text| clean_label(text))
            else {
                continue;
            };
            if fresh != computer.label {
                computer.label.clone_from(&fresh);
                if let Ok(computer_name) = ComputerName::parse(name) {
                    changed.push((computer_name, fresh));
                }
            }
        }
        if changed.is_empty() {
            return false;
        }
        // The names shown change even if the file could not be written; the next change retries.
        let _ = self.save(&next);
        state.file = next;
        for (name, label) in changed {
            self.book.set_label(name, label);
        }
        true
    }

    /// Removes a computer that was added here: its models, its keys, its place in the file. The
    /// name is the computer's name or its place id (`computer:<name>`). One written by hand in the
    /// settings file is refused. The node id of a computer on the Tailscale network, which the
    /// caller stops the relay of.
    pub fn remove(&self, name: &str) -> Result<Option<NodeId>, ComputerError> {
        let text = name.strip_prefix("computer:").unwrap_or(name);
        let name = ComputerName::parse(text).map_err(|_| ComputerError::NotThere)?;
        let mut state = self.state();
        let Some(gone) = state.file.computers.get(name.as_str()).cloned() else {
            return Err(if state.by_hand.contains(&name) {
                ComputerError::AddedByHand
            } else {
                ComputerError::NotThere
            });
        };
        let mut next = state.file.clone();
        next.computers.remove(name.as_str());
        self.save(&next)?;
        state.file = next;
        self.book.remove_computer(&name);
        let keys: Vec<PathBuf> = gone
            .models
            .values()
            .filter_map(|model| model.key_file.clone())
            .collect();
        forget(&keys.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        Ok(gone
            .node
            .as_deref()
            .and_then(|node| NodeId::parse(node).ok()))
    }
}

/// Removes key files that are no longer wanted; one already gone is nothing to say.
fn forget(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests;

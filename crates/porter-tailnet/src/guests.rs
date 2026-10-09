//! The computers the person said yes or no to using this computer's models, and the ones that
//! are waiting for an answer.
//!
//! A record is kept by the computer's stable id ([`NodeId`]) and nothing else: the name is only
//! what the person is shown, so a computer renamed on the network is still the one they
//! answered about, and a different computer given the old one's name or address is not. The
//! records are a file only the owner reads (`AtomicWrite::PRIVATE`); a file that cannot be read
//! is put aside and the list starts empty, so a later change does not write over what the
//! person may want back. The questions still waiting are kept in memory only: after a restart
//! the computer asks again when it next wants a model.

use porter_core::atomic::AtomicWrite;
use porter_core::{NodeId, UnixSeconds};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

/// The most records kept. The person's own computers are a handful.
const MOST_GUESTS: usize = 256;

/// The most computers waiting for an answer at once. A computer asking more is refused until the
/// person has answered some, so the questions cannot pile up without end.
const MOST_ASKS: usize = 16;

/// The longest name kept, in characters.
const LONGEST_NAME: usize = 64;

/// What the person said about a computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// It may use this computer's models.
    Approved,
    /// It may not, and is not asked about again.
    Denied,
}

impl State {
    /// The word on the bus and in the file.
    pub fn slug(self) -> &'static str {
        match self {
            State::Approved => "approved",
            State::Denied => "denied",
        }
    }
}

/// The person's answer to a computer: what `AnswerGuest` takes, as a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuestAnswer {
    /// Let it use this computer's models.
    Allow,
    /// Keep it out.
    Deny,
}

impl GuestAnswer {
    /// Both answers, for tables.
    pub const ALL: [GuestAnswer; 2] = [GuestAnswer::Allow, GuestAnswer::Deny];

    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            GuestAnswer::Allow => "allow",
            GuestAnswer::Deny => "deny",
        }
    }

    /// The answer a word names; none for any other word.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|answer| answer.slug() == slug)
    }
}

/// One record, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guest {
    /// The name the computer had when the person answered.
    pub name: String,
    /// The answer.
    pub state: State,
    /// When the person answered.
    pub since: UnixSeconds,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    guests: BTreeMap<String, Guest>,
}

/// A computer waiting for an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Its stable id.
    pub node: NodeId,
    /// Its name now.
    pub name: String,
    /// When it first asked (since this computer started).
    pub since: UnixSeconds,
}

/// What asking did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskOutcome {
    /// The computer was not waiting yet: the person is to be asked now.
    Raised,
    /// It already was; nothing new to tell the person.
    Pending,
}

/// What changed, for whoever tells the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestEvent {
    /// A computer began waiting for an answer.
    Asked(Ask),
    /// The records or the waiting list changed (an answer, a record forgotten, a question
    /// raised or withdrawn).
    Changed,
}

/// Why a change was not made. `Display` is the plain sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GuestError {
    /// The computer is not waiting for an answer.
    #[error("That computer is not asking to use this computer's models.")]
    NotAsking,
    /// Too many computers are waiting for an answer.
    #[error("Too many computers are asking at once. Answer some of them first.")]
    TooManyAsking,
    /// The list is as long as it may be.
    #[error("That is more computers than can be kept here.")]
    TooMany,
    /// The answer could not be written down.
    #[error("The answer could not be saved.")]
    NotSaved,
}

/// How one computer stands, as the list shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// The person said yes.
    Approved,
    /// The person said no.
    Denied,
    /// It is waiting for an answer.
    Asking,
}

impl RowState {
    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            RowState::Approved => "approved",
            RowState::Denied => "denied",
            RowState::Asking => "asking",
        }
    }
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRow {
    /// The computer's stable id.
    pub node: NodeId,
    /// Its name.
    pub name: String,
    /// How it stands.
    pub state: RowState,
    /// When it was answered, or began asking.
    pub since: UnixSeconds,
}

#[derive(Debug, Default)]
struct Inner {
    file: File,
    asking: BTreeMap<NodeId, Ask>,
}

type Listener = Box<dyn Fn(&GuestEvent) + Send + Sync>;

/// The computers' answers and questions of one lending computer.
pub struct Guests {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
    listeners: RwLock<Vec<Arc<Listener>>>,
}

impl std::fmt::Debug for Guests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guests")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The name kept for a computer: trimmed, without control characters, cut to a sane length.
fn clean(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(LONGEST_NAME)
        .collect()
}

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section is a plain update of the data.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Guests {
    /// Records kept nowhere: for a computer that is not to remember, and for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            inner: Mutex::default(),
            listeners: RwLock::default(),
        }
    }

    /// The records kept in the file at `path`. A missing file is the empty list. One that cannot
    /// be read as a list is moved aside (`<name>.unusable`) and the list starts empty; the
    /// second part says so, in a line for a log. A record whose id is not a computer's is
    /// dropped.
    pub fn open(path: &Path) -> (Self, Option<String>) {
        let (file, said) = match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<File>(&text) {
                Ok(file) => (file, None),
                Err(_) => {
                    let mut aside = path.as_os_str().to_owned();
                    aside.push(".unusable");
                    let aside = PathBuf::from(aside);
                    let _ = std::fs::rename(path, &aside);
                    (
                        File::default(),
                        Some(format!(
                            "{}: is not a list of computers; kept as {} and started empty",
                            path.display(),
                            aside.display()
                        )),
                    )
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (File::default(), None),
            Err(e) => (
                File::default(),
                Some(format!(
                    "{}: cannot be read ({:?})",
                    path.display(),
                    e.kind()
                )),
            ),
        };
        let file = File {
            guests: file
                .guests
                .into_iter()
                .filter(|(node, _)| NodeId::parse(node).is_ok())
                .take(MOST_GUESTS)
                .collect(),
        };
        (
            Self {
                path: Some(path.to_owned()),
                inner: Mutex::new(Inner {
                    file,
                    asking: BTreeMap::new(),
                }),
                listeners: RwLock::default(),
            },
            said,
        )
    }

    /// Calls `listener` after every change, from the thread that made it (it is to hand the
    /// news on, not to work).
    pub fn on_event(&self, listener: impl Fn(&GuestEvent) + Send + Sync + 'static) {
        self.listeners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(Box::new(listener)));
    }

    fn tell(&self, events: &[GuestEvent]) {
        let listeners = self
            .listeners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for event in events {
            for listener in &listeners {
                listener(event);
            }
        }
    }

    /// What the person said of `node`, when they did.
    pub fn state_of(&self, node: &NodeId) -> Option<State> {
        held(&self.inner)
            .file
            .guests
            .get(node.as_str())
            .map(|guest| guest.state)
    }

    fn save(&self, file: &File) -> Result<(), GuestError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string_pretty(file).map_err(|_| GuestError::NotSaved)?;
        AtomicWrite::PRIVATE
            .write(path, text.as_bytes())
            .map_err(|_| GuestError::NotSaved)
    }

    /// Notes that the computer `node`, called `name`, wants to use the models and has not been
    /// answered: [`AskOutcome::Raised`] the first time, so the person is told once.
    pub fn ask(
        &self,
        node: &NodeId,
        name: &str,
        now: UnixSeconds,
    ) -> Result<AskOutcome, GuestError> {
        let ask = {
            let mut inner = held(&self.inner);
            if let Some(waiting) = inner.asking.get_mut(node) {
                // The name may have changed since it first asked; the person is shown the new one.
                let name = clean(name);
                if !name.is_empty() {
                    waiting.name = name;
                }
                return Ok(AskOutcome::Pending);
            }
            if inner.asking.len() >= MOST_ASKS {
                return Err(GuestError::TooManyAsking);
            }
            let name = match clean(name) {
                name if name.is_empty() => node.to_string(),
                name => name,
            };
            let ask = Ask {
                node: node.clone(),
                name,
                since: now,
            };
            inner.asking.insert(node.clone(), ask.clone());
            ask
        };
        self.tell(&[GuestEvent::Asked(ask), GuestEvent::Changed]);
        Ok(AskOutcome::Raised)
    }

    /// The person's answer to a computer that is waiting: yes (`allow`) or no.
    pub fn answer(
        &self,
        node: &NodeId,
        answer: GuestAnswer,
        now: UnixSeconds,
    ) -> Result<(), GuestError> {
        {
            let mut inner = held(&self.inner);
            let ask = inner.asking.get(node).ok_or(GuestError::NotAsking)?.clone();
            let mut next = inner.file.clone();
            if next.guests.len() >= MOST_GUESTS && !next.guests.contains_key(node.as_str()) {
                return Err(GuestError::TooMany);
            }
            next.guests
                .insert(node.to_string(), record(&ask.name, answer, now));
            self.save(&next)?;
            inner.file = next;
            inner.asking.remove(node);
        }
        self.tell(&[GuestEvent::Changed]);
        Ok(())
    }

    /// The person's word about a computer, waiting or not (Settings only): it may be one this
    /// computer would never ask about, such as a server or another person's computer.
    pub fn set(
        &self,
        node: &NodeId,
        name: &str,
        answer: GuestAnswer,
        now: UnixSeconds,
    ) -> Result<(), GuestError> {
        {
            let mut inner = held(&self.inner);
            let mut next = inner.file.clone();
            if next.guests.len() >= MOST_GUESTS && !next.guests.contains_key(node.as_str()) {
                return Err(GuestError::TooMany);
            }
            // The name given, else the one it asked under, else its id.
            let name = Some(clean(name))
                .filter(|name| !name.is_empty())
                .or_else(|| inner.asking.get(node).map(|ask| ask.name.clone()))
                .unwrap_or_else(|| node.to_string());
            next.guests
                .insert(node.to_string(), record(&name, answer, now));
            self.save(&next)?;
            inner.file = next;
            inner.asking.remove(node);
        }
        self.tell(&[GuestEvent::Changed]);
        Ok(())
    }

    /// Forgets what the person said about `node`, and any question it has waiting: it is asked
    /// about again the next time it wants a model. Whether there was anything to forget.
    pub fn forget(&self, node: &NodeId) -> Result<bool, GuestError> {
        let changed = {
            let mut inner = held(&self.inner);
            let mut next = inner.file.clone();
            let had_record = next.guests.remove(node.as_str()).is_some();
            if had_record {
                self.save(&next)?;
                inner.file = next;
            }
            let had_ask = inner.asking.remove(node).is_some();
            had_record || had_ask
        };
        if changed {
            self.tell(&[GuestEvent::Changed]);
        }
        Ok(changed)
    }

    /// The computers waiting for an answer, oldest first.
    pub fn asking(&self) -> Vec<Ask> {
        let mut asks: Vec<Ask> = held(&self.inner).asking.values().cloned().collect();
        asks.sort_by_key(|ask| ask.since);
        asks
    }

    /// Every computer the person answered or is being asked about, by name.
    pub fn rows(&self) -> Vec<GuestRow> {
        let inner = held(&self.inner);
        let mut rows: Vec<GuestRow> = inner
            .file
            .guests
            .iter()
            .filter_map(|(node, guest)| {
                Some(GuestRow {
                    node: NodeId::parse(node).ok()?,
                    name: guest.name.clone(),
                    state: match guest.state {
                        State::Approved => RowState::Approved,
                        State::Denied => RowState::Denied,
                    },
                    since: guest.since,
                })
            })
            .collect();
        rows.extend(inner.asking.values().map(|ask| GuestRow {
            node: ask.node.clone(),
            name: ask.name.clone(),
            state: RowState::Asking,
            since: ask.since,
        }));
        rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.node.cmp(&b.node)));
        rows
    }
}

fn record(name: &str, answer: GuestAnswer, now: UnixSeconds) -> Guest {
    Guest {
        name: name.to_owned(),
        state: match answer {
            GuestAnswer::Allow => State::Approved,
            GuestAnswer::Deny => State::Denied,
        },
        since: now,
    }
}

#[cfg(test)]
mod tests;

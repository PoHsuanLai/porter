//! One entry of a [`ChunkedStore`]: a head in the wrapped store, and the parts it names.

use crate::store::ChunkedStore;
use crate::{ChunkError, Limit, MOST_PARTS, pieces};
use keyring_core::api::{CredentialApi, CredentialStoreApi};
use keyring_core::{Credential, Entry, Error, Result};
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// How a value goes into an entry of the wrapped store: as a password (UTF-16 on Windows) or as
/// bytes. A value is read back the way it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Password,
    Secret,
}

/// The name of part `index` (from 1) of the value under `user`.
fn part_name(user: &str, index: usize) -> String {
    format!("{user}#{index}")
}

/// Whether `user` names a part: it ends in `#` and a number.
pub(crate) fn is_part(user: &str) -> bool {
    user.rsplit_once('#')
        .is_some_and(|(_, index)| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
}

/// A damaged head or a missing part, as keyring-core reports data it cannot read.
fn unreadable(error: ChunkError) -> Error {
    Error::BadDataFormat(Vec::new(), Box::new(error))
}

/// A credential of a [`ChunkedStore`]: the entry `user` under `service`, and its parts
/// `user#1`, `user#2` and so on when the value is too long for one.
pub struct ChunkedCredential<S: ?Sized> {
    store: ChunkedStore<S>,
    service: String,
    user: String,
    modifiers: Option<Vec<(String, String)>>,
}

impl<S: ?Sized> fmt::Debug for ChunkedCredential<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkedCredential")
            .field("service", &self.service)
            .field("user", &self.user)
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl<S: CredentialStoreApi + Send + Sync + ?Sized + 'static> ChunkedCredential<S> {
    pub(crate) fn new(
        store: ChunkedStore<S>,
        service: &str,
        user: &str,
        modifiers: Option<&HashMap<&str, &str>>,
    ) -> Self {
        Self {
            store,
            service: service.to_owned(),
            user: user.to_owned(),
            modifiers: modifiers.map(|m| {
                m.iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect()
            }),
        }
    }

    /// The wrapped store's entry `user`: the head, or a part.
    fn entry(&self, user: &str) -> Result<Entry> {
        let modifiers: Option<HashMap<&str, &str>> = self
            .modifiers
            .as_ref()
            .map(|m| m.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect());
        self.store
            .inner
            .build(&self.service, user, modifiers.as_ref())
    }

    /// The value in the wrapped entry `user`, or `None` when there is none.
    fn read(&self, user: &str, form: Form) -> Result<Option<Vec<u8>>> {
        let entry = self.entry(user)?;
        let got = match form {
            Form::Password => entry.get_password().map(String::into_bytes),
            Form::Secret => entry.get_secret(),
        };
        match got {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write(&self, user: &str, form: Form, value: &[u8]) -> Result<()> {
        let entry = self.entry(user)?;
        match form {
            Form::Password => {
                // Every piece is cut at a character boundary of a `&str`.
                let text =
                    std::str::from_utf8(value).map_err(|_| Error::BadEncoding(Vec::new()))?;
                entry.set_password(text)
            }
            Form::Secret => entry.set_secret(value),
        }
    }

    /// Removes the wrapped entry `user`; whether there was one.
    fn remove(&self, user: &str) -> Result<bool> {
        match self.entry(user)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(Error::NoEntry) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The part count a head names, `None` for a value stored whole.
    fn parts_in(&self, head: &[u8]) -> std::result::Result<Option<usize>, ChunkError> {
        let Some(count) = head.strip_prefix(self.store.marker.as_bytes()) else {
            return Ok(None);
        };
        let count = std::str::from_utf8(count).map_err(|_| ChunkError::BadHead)?;
        match count.parse::<usize>() {
            Ok(count) if (2..=MOST_PARTS).contains(&count) => Ok(Some(count)),
            _ => Err(ChunkError::BadHead),
        }
    }

    /// How many parts the value is stored in: 0 for none or a value stored whole. A damaged
    /// head is the [`ChunkError`]; a store that cannot be read is the outer error.
    fn count(&self, form: Form) -> Result<std::result::Result<usize, ChunkError>> {
        Ok(match self.read(&self.user, form)? {
            Some(head) => self.parts_in(&head).map(|parts| parts.unwrap_or(0)),
            None => Ok(0),
        })
    }

    /// Removes parts from `from` on: every one up to `through`, then on while there are more,
    /// so a run an earlier write left behind goes too.
    fn remove_parts(&self, from: usize, through: usize) -> Result<()> {
        let mut index = from;
        loop {
            let was = self.remove(&part_name(&self.user, index))?;
            if !was && index > through {
                return Ok(());
            }
            if index >= MOST_PARTS && index >= through {
                return Ok(());
            }
            index += 1;
        }
    }

    fn put(&self, form: Form, value: &[u8]) -> Result<()> {
        // A damaged head names no count; the sweep after the write still finds its parts.
        let before = self.count(form)?.unwrap_or(0);
        let mut parts: Vec<&[u8]> = match form {
            Form::Password => {
                let text =
                    std::str::from_utf8(value).map_err(|_| Error::BadEncoding(value.to_vec()))?;
                pieces::text(text, self.store.limit)
                    .into_iter()
                    .map(str::as_bytes)
                    .collect()
            }
            Form::Secret => pieces::bytes(value, self.store.limit),
        };
        // A value that begins with the marker would read as a head: it goes in parts.
        if parts.len() == 1 && value.starts_with(self.store.marker.as_bytes()) {
            parts.push(&[]);
        }
        if parts.len() > MOST_PARTS {
            let most = match self.store.limit {
                Limit::Utf16Units(units) => units.saturating_mul(MOST_PARTS),
                Limit::None => usize::MAX,
            };
            return Err(Error::TooLong(
                self.user.clone(),
                u32::try_from(most).unwrap_or(u32::MAX),
            ));
        }
        let now = match parts.as_slice() {
            [whole] => {
                self.write(&self.user, form, whole)?;
                0
            }
            parts => {
                for (at, part) in parts.iter().enumerate() {
                    if let Err(e) = self.write(&part_name(&self.user, at + 1), form, part) {
                        self.undo(before, at);
                        return Err(e);
                    }
                }
                let head = format!("{}{}", self.store.marker, parts.len());
                if let Err(e) = self.write(&self.user, form, head.as_bytes()) {
                    self.undo(before, parts.len());
                    return Err(e);
                }
                parts.len()
            }
        };
        self.remove_parts(now + 1, before)
    }

    /// After a write that failed with `written` parts of the new value in place: nothing to do
    /// when none were; the new parts go when the old value was whole (it is still there); and
    /// when the old value was in parts, some of them overwritten, all of it goes, so it reads
    /// as missing and not as a mix. Best effort: the write's own error is what is returned.
    fn undo(&self, before: usize, written: usize) {
        if written == 0 {
            return;
        }
        if before > 0 {
            let _ = self.remove(&self.user);
        }
        let _ = self.remove_parts(1, written.max(before));
    }

    fn get(&self, form: Form) -> Result<Vec<u8>> {
        let head = self.read(&self.user, form)?.ok_or(Error::NoEntry)?;
        let Some(count) = self.parts_in(&head).map_err(unreadable)? else {
            return Ok(head);
        };
        let mut value = Vec::new();
        for index in 1..=count {
            match self.read(&part_name(&self.user, index), form)? {
                Some(part) => value.extend_from_slice(&part),
                None => return Err(unreadable(ChunkError::MissingPart { index, count })),
            }
        }
        Ok(value)
    }

    fn forget(&self) -> Result<()> {
        // A damaged head still has to go, and so do the parts it may have had. The head is read
        // in both forms, as either may have written it; one that does not read at all leaves
        // the sweep to find the parts.
        let parts = [Form::Password, Form::Secret]
            .into_iter()
            .map(|form| match self.count(form) {
                Ok(Ok(parts)) => parts,
                Ok(Err(_)) => MOST_PARTS,
                Err(_) => 0,
            })
            .max()
            .unwrap_or(0);
        self.remove_parts(1, parts)?;
        if self.remove(&self.user)? {
            Ok(())
        } else {
            Err(Error::NoEntry)
        }
    }
}

impl<S: CredentialStoreApi + Send + Sync + ?Sized + 'static> CredentialApi
    for ChunkedCredential<S>
{
    fn set_password(&self, password: &str) -> Result<()> {
        self.put(Form::Password, password.as_bytes())
    }

    fn set_secret(&self, secret: &[u8]) -> Result<()> {
        self.put(Form::Secret, secret)
    }

    fn get_password(&self) -> Result<String> {
        let value = self.get(Form::Password)?;
        String::from_utf8(value).map_err(|e| Error::BadEncoding(e.into_bytes()))
    }

    fn get_secret(&self) -> Result<Vec<u8>> {
        self.get(Form::Secret)
    }

    /// The head entry's attributes.
    fn get_attributes(&self) -> Result<HashMap<String, String>> {
        self.entry(&self.user)?.get_attributes()
    }

    /// Updates the head entry's attributes.
    fn update_attributes(&self, attributes: &HashMap<&str, &str>) -> Result<()> {
        self.entry(&self.user)?.update_attributes(attributes)
    }

    /// Removes the head and every part. [`Error::NoEntry`] when there was no head.
    fn delete_credential(&self) -> Result<()> {
        self.forget()
    }

    /// `None` (this entry is its own wrapper) once the head is there; [`Error::NoEntry`] when
    /// it is not.
    fn get_credential(&self) -> Result<Option<Arc<Credential>>> {
        self.entry(&self.user)?.get_credential()?;
        Ok(None)
    }

    fn get_specifiers(&self) -> Option<(String, String)> {
        Some((self.service.clone(), self.user.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

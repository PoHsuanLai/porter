//! `org.quire.Photos1.Picker` on syncd's object (`porter_dbus::PhotosPickerProxy`): the Google
//! Photos Picker for the Photos app. Served only while syncd runs Google Photos
//! (`SYNCD_PHOTOS`); the caller is identified as `Sync1`'s are (the connection's, by
//! porter-dbus's `Callers`), and only an app that owns the Photos datasets (`org.quire.Photos`)
//! may call it. Settings and the porter daemons see datasets but are not the app that imports
//! photos, so they are `Denied`, as they are for `Sync1.Resolve`.
//!
//! - An account with no Picker (unknown, not Google, the person did not grant the picker scope,
//!   Photos not granted to syncd) answers `NoFittingAccount`.
//! - A malformed account or session word is `InvalidArgs`.
//! - `Import` before the person is done is `org.quire.Photos1.Error.NotYet`; a session Google
//!   does not have is `org.quire.Photos1.Error.NoSuchSession`; the Picker not answering is
//!   `Unavailable`; anything else `Failed`.
//! - Nothing but ids, the page to open, a state word and the files' paths crosses the bus.

use super::errors::RefusedError;
use super::hub::Access;
use crate::datasets::photos::google::{GooglePicker, PickerError, PickerState, SessionId};
use crate::paths::AccountDir;
use porter_client::Transport;
use porter_core::wire::Refusal;
use porter_dbus::{Callers, PICKER_PICKED, PICKER_WAITING, SYNC_PATH};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use zbus::Connection;
use zbus::message::Header;

/// The Pickers of the accounts syncd runs Google Photos for, shared between the supervisor
/// that starts them and the bus object that serves them.
pub struct Pickers<T: Transport>(Arc<Mutex<BTreeMap<AccountDir, Arc<GooglePicker<T>>>>>);

impl<T: Transport> Clone for Pickers<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Transport> Default for Pickers<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T: Transport> std::fmt::Debug for Pickers<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.accounts()).finish()
    }
}

impl<T: Transport> Pickers<T> {
    /// Replaces the set with `now`.
    pub fn set(&self, now: BTreeMap<AccountDir, Arc<GooglePicker<T>>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = now;
    }

    fn accounts(&self) -> Vec<AccountDir> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    fn of(&self, account: &AccountDir) -> Option<Arc<GooglePicker<T>>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account)
            .cloned()
    }
}

/// What a session looks like to the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The session id.
    pub session: String,
    /// The page the app opens.
    pub picker_uri: String,
    /// Seconds between polls.
    pub poll_s: u32,
}

/// The Picker of an account, as the bus object asks for it.
pub trait PickerDesk: Send + Sync + 'static {
    /// Starts a session; `None` when the account has no Picker.
    fn start(
        &self,
        account: &AccountDir,
    ) -> impl Future<Output = Option<Result<Started, PickerError>>> + Send;
    /// The session's state; `None` when the account has no Picker.
    fn poll(
        &self,
        account: &AccountDir,
        session: &SessionId,
    ) -> impl Future<Output = Option<Result<PickerState, PickerError>>> + Send;
    /// Imports what was picked: the files' paths; `None` when the account has no Picker.
    fn import(
        &self,
        account: &AccountDir,
        session: &SessionId,
    ) -> impl Future<Output = Option<Result<Vec<String>, PickerError>>> + Send;
    /// Ends the session; whether the account has a Picker.
    fn cancel(
        &self,
        account: &AccountDir,
        session: &SessionId,
    ) -> impl Future<Output = bool> + Send;
}

impl<T: Transport + 'static> PickerDesk for Pickers<T> {
    async fn start(&self, account: &AccountDir) -> Option<Result<Started, PickerError>> {
        let picker = self.of(account)?;
        Some(picker.start().await.map(|s| Started {
            session: s.id.to_string(),
            picker_uri: s.picker_uri,
            poll_s: u32::try_from(s.poll_every.as_secs().max(1)).unwrap_or(u32::MAX),
        }))
    }

    async fn poll(
        &self,
        account: &AccountDir,
        session: &SessionId,
    ) -> Option<Result<PickerState, PickerError>> {
        Some(self.of(account)?.poll(session).await)
    }

    async fn import(
        &self,
        account: &AccountDir,
        session: &SessionId,
    ) -> Option<Result<Vec<String>, PickerError>> {
        let picker = self.of(account)?;
        Some(picker.import(session).await.map(|imported| {
            imported
                .files
                .iter()
                .map(|f| f.to_string_lossy().into_owned())
                .collect()
        }))
    }

    async fn cancel(&self, account: &AccountDir, session: &SessionId) -> bool {
        match self.of(account) {
            Some(picker) => {
                picker.cancel(session).await;
                true
            }
            None => false,
        }
    }
}

struct Core<C, D> {
    callers: C,
    desk: D,
    owners: Access,
}

impl<C: Callers, D: PickerDesk> Core<C, D> {
    /// Checks the caller is the Photos app, and the words.
    async fn admit(
        &self,
        header: &Header<'_>,
        account: &str,
        session: Option<&str>,
    ) -> Result<(AccountDir, Option<SessionId>), RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("syncd does not know this caller"))?;
        if !self.owners.owned_by(&caller) {
            return Err(RefusedError::of(Refusal::Denied));
        }
        let account = AccountDir::parse(account)
            .ok_or_else(|| RefusedError::invalid("not an account directory name"))?;
        let session = session
            .map(|text| {
                SessionId::parse(text)
                    .ok_or_else(|| RefusedError::invalid("the session id is not a plain token"))
            })
            .transpose()?;
        Ok((account, session))
    }
}

fn refused(error: PickerError) -> RefusedError {
    match error {
        PickerError::NotYet => RefusedError::picker_not_yet(),
        PickerError::Refused(404) => RefusedError::picker_no_such_session(),
        PickerError::Unreached => RefusedError::of(Refusal::Unavailable),
        PickerError::BadSession => RefusedError::invalid("the session id is not a plain token"),
        other => RefusedError::failed(other),
    }
}

fn none_here() -> RefusedError {
    RefusedError::of(Refusal::NoFittingAccount)
}

struct PickerObject<C, D>(Arc<Core<C, D>>);

#[zbus::interface(name = "org.quire.Photos1.Picker")]
impl<C: Callers, D: PickerDesk> PickerObject<C, D> {
    async fn start(
        &self,
        #[zbus(header)] header: Header<'_>,
        account: String,
    ) -> Result<(String, String, u32), RefusedError> {
        let (account, _) = self.0.admit(&header, &account, None).await?;
        let started = self
            .0
            .desk
            .start(&account)
            .await
            .ok_or_else(none_here)?
            .map_err(refused)?;
        Ok((started.session, started.picker_uri, started.poll_s))
    }

    async fn poll(
        &self,
        #[zbus(header)] header: Header<'_>,
        account: String,
        session: String,
    ) -> Result<String, RefusedError> {
        let (account, session) = self.0.admit(&header, &account, Some(&session)).await?;
        let session = session.ok_or_else(none_here)?;
        let state = self
            .0
            .desk
            .poll(&account, &session)
            .await
            .ok_or_else(none_here)?
            .map_err(refused)?;
        Ok(match state {
            PickerState::Waiting => PICKER_WAITING,
            PickerState::Picked => PICKER_PICKED,
        }
        .to_owned())
    }

    async fn import(
        &self,
        #[zbus(header)] header: Header<'_>,
        account: String,
        session: String,
    ) -> Result<Vec<String>, RefusedError> {
        let (account, session) = self.0.admit(&header, &account, Some(&session)).await?;
        let session = session.ok_or_else(none_here)?;
        self.0
            .desk
            .import(&account, &session)
            .await
            .ok_or_else(none_here)?
            .map_err(refused)
    }

    async fn cancel(
        &self,
        #[zbus(header)] header: Header<'_>,
        account: String,
        session: String,
    ) -> Result<(), RefusedError> {
        let (account, session) = self.0.admit(&header, &account, Some(&session)).await?;
        let session = session.ok_or_else(none_here)?;
        match self.0.desk.cancel(&account, &session).await {
            true => Ok(()),
            false => Err(none_here()),
        }
    }
}

/// Serves `org.quire.Photos1.Picker` beside `Sync1` on `connection`: `desk` has the Pickers,
/// `owners` is who may call (the Photos app), `callers` says who the senders are.
pub async fn serve_picker<C: Callers, D: PickerDesk>(
    connection: &Connection,
    desk: D,
    owners: Access,
    callers: C,
) -> zbus::Result<()> {
    let core = Arc::new(Core {
        callers,
        desk,
        owners,
    });
    connection
        .object_server()
        .at(SYNC_PATH, PickerObject(core))
        .await?;
    Ok(())
}

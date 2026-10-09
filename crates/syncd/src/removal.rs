//! `AccountRemoved`: when accountd says an account is gone, syncd forgets everything of it:
//! its running datasets, its journals and anchors (`<state>/porter/sync/<account>`), its
//! mirrors (`<data>/porter/vdir/<account>`) and its photos library (`<data>/porter/photos/<account>`) and its app folder mirror
//! (`<data>/porter/storage/<account>`) (PLAN §2.9: "syncd | journal rows, anchors and
//! mirrors of that account").
//!
//! The signal is accountd's, unicast to the apps that hold a grant for the account;
//! porter-client's [`Accounts::watch_removals`] matches the sender against the owner of
//! `org.quire.Accounts1`, so a signal another connection sends does not wipe anything, and
//! introduces syncd to accountd at the start and at every new owner (accountd tells only the
//! connections that have called it). The account arrives as its object path's last segment,
//! which is the directory name (`AccountDir`); anything that is not such a name is ignored.

use crate::paths::{AccountDir, Paths};
use crate::service::Hub;
use porter_client::{Accounts, ClientError, DbusTransport};
use std::io::ErrorKind;
use zbus::Connection;

/// Stops the account's datasets, waits until none is in a cycle, and deletes its journals and
/// mirrors; how many directories existed. A directory already gone is not an error.
pub async fn wipe(paths: &Paths, hub: &Hub, account: &AccountDir) -> std::io::Result<usize> {
    hub.stop_account(account).await;
    let dirs: Vec<_> = paths
        .account_dirs(account)
        .into_iter()
        .chain([paths.photos_dir(account), paths.storage_dir(account)])
        .collect();
    // Mirrors and libraries hold many files: removing them blocks, so it runs off the async
    // threads (the daemon's other accounts keep being served meanwhile).
    tokio::task::spawn_blocking(move || {
        let mut removed = 0;
        for dir in dirs {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(removed)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Listens for the accounts accountd removes and wipes them. A directory that cannot be removed
/// is reported on standard error (the daemon's one log path) and the next removal is awaited.
pub async fn watch(connection: &Connection, hub: Hub, paths: Paths) -> Result<(), ClientError> {
    let mut removals = Accounts::over(DbusTransport::over(connection.clone()))
        .watch_removals()
        .await?;
    tokio::spawn(async move {
        while let Some(removed) = removals.next().await {
            let Some(account) = AccountDir::parse(removed.segment()) else {
                continue;
            };
            if let Err(why) = wipe(&paths, &hub, &account).await {
                eprintln!("syncd: cannot wipe account {account}: {why}");
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn a_wipe_removes_that_accounts_journals_and_mirrors_and_nothing_else() {
        let root = scratch("wipe");
        let paths = Paths {
            journals: root.join("state/porter/sync"),
            mirrors: root.join("data/porter/vdir"),
            callers_system: root.join("none"),
            callers_user: root.join("none"),
        };
        let (gone, kept) = (
            AccountDir::parse("a1").expect("a1"),
            AccountDir::parse("a2").expect("a2"),
        );
        for (account, file) in [(&gone, "files.sqlite"), (&kept, "files.sqlite")] {
            let journal = paths.journal(account, "files");
            std::fs::create_dir_all(journal.parent().expect("dir")).expect("dir");
            std::fs::write(&journal, file).expect("journal");
            let [_, mirror] = paths.account_dirs(account);
            std::fs::create_dir_all(mirror.join("calendar")).expect("mirror");
            std::fs::write(mirror.join("calendar/x.ics"), "BEGIN:VCALENDAR").expect("ics");
        }
        let hub = Hub::default();
        let name = crate::service::DatasetName::parse("a1/files").expect("name");
        let handle = hub.register(name, crate::service::Access::default());

        assert_eq!(wipe(&paths, &hub, &gone).await.expect("wipe"), 2);
        let left: BTreeSet<_> = [&paths.journals, &paths.mirrors]
            .iter()
            .flat_map(|dir| std::fs::read_dir(dir).expect("dir"))
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, BTreeSet::from(["a2".to_owned()]));
        assert!(!handle.is_registered(), "its engines are stopped");
        assert_eq!(
            wipe(&paths, &hub, &gone).await.expect("again"),
            0,
            "idempotent"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

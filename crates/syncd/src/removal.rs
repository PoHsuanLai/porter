//! Removing an account: syncd forgets everything of it: its running datasets, its journals and
//! anchors (`<state>/porter/sync/<account>`, the drive listing's `storage_drive` among them), its
//! mirrors (`<data>/porter/vdir/<account>`), its
//! photos library (`<data>/porter/photos/<account>`) and its app folder mirror
//! (`<data>/porter/storage/<account>`), and its drive's downloaded files
//! (`<cache>/porter/drives/<account>`).
//!
//! The account is syncd's own, removed in its Settings rows (`crate::keep::Keep::remove` calls
//! [`wipe`] and then deletes the account's stored sign-in). accountd removing an identity does
//! not remove syncd's copy: the person removes it in the sync rows.

use crate::paths::{AccountDir, Paths};
use crate::service::Hub;
use std::io::ErrorKind;

/// Stops the account's datasets, waits until none is in a cycle, and deletes its journals and
/// mirrors; how many directories existed. A directory already gone is not an error.
pub async fn wipe(paths: &Paths, hub: &Hub, account: &AccountDir) -> std::io::Result<usize> {
    hub.stop_account(account).await;
    let dirs: Vec<_> = paths
        .account_dirs(account)
        .into_iter()
        .chain([
            paths.photos_dir(account),
            paths.storage_dir(account),
            paths.drive_cache_dir(account),
        ])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn a_wipe_removes_that_accounts_journals_and_mirrors_and_nothing_else() {
        let _root_guard = scratch("wipe");
        let root = _root_guard.path().to_owned();
        let paths = Paths {
            journals: root.join("state/porter/sync"),
            mirrors: root.join("data/porter/vdir"),
            callers_system: root.join("none"),
            callers_user: root.join("none"),
            cloud: root.join("Cloud"),
            drive_cache: root.join("cache/porter/drives"),
            settings: root.join("none"),
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
        let drive = crate::datasets::drive::journal_path(&paths, &gone);
        crate::datasets::drive::DriveJournal::open(&drive).expect("the drive's journal");
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
        assert!(!drive.exists(), "the drive listing goes with the rest");
        assert_eq!(
            wipe(&paths, &hub, &gone).await.expect("again"),
            0,
            "idempotent"
        );
    }
}

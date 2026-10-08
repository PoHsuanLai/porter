//! `AccountRemoved`: when accountd says an account is gone, syncd forgets everything of it:
//! its running datasets, its journals and anchors (`<state>/porter/sync/<account>`), its
//! mirrors (`<data>/porter/vdir/<account>`) and its photos library (`<data>/porter/photos/<account>`) and its app folder mirror
//! (`<data>/porter/storage/<account>`) (PLAN §2.9: "syncd | journal rows, anchors and
//! mirrors of that account").
//!
//! The signal is accountd's, unicast to the apps that hold a grant for the account; the proxy
//! matches the sender against the owner of `org.quire.Accounts1`, so a signal another connection
//! sends does not wipe anything. The account arrives as its object path, whose last segment is
//! the directory name (`AccountDir`); anything that is not such a path is ignored.
//!
//! accountd tells only the connections that have called it, so syncd calls it once at start and
//! again whenever accountd gets a new owner on the bus (`Grants.List`, the caller's own grants):
//! without that a restarted accountd would not know there is a syncd to tell.

use crate::paths::{AccountDir, Paths};
use crate::service::Hub;
use porter_dbus::{ACCOUNTS_BUS, GrantsProxy, ManagerProxy};
use std::io::ErrorKind;
use std::pin::Pin;
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;

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

/// Listens for `AccountRemoved` on `connection` and wipes. A directory that cannot be removed
/// is reported on standard error (the daemon's one log path) and the next signal is awaited.
pub async fn watch(connection: &Connection, hub: Hub, paths: Paths) -> zbus::Result<()> {
    let mut removals = ManagerProxy::new(connection)
        .await?
        .receive_account_removed()
        .await?;
    let mut owners = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed_with_args(&[(0, ACCOUNTS_BUS)])
        .await?;
    let announce = connection.clone();
    tokio::spawn(async move {
        introduce(&announce).await;
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await
        {
            if signal.args().is_ok_and(|args| args.new_owner().is_some()) {
                introduce(&announce).await;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut removals).poll_next(cx)).await
        {
            let Ok(args) = signal.args() else { continue };
            let Some(account) = AccountDir::of_object_path(args.account().as_str()) else {
                continue;
            };
            if let Err(why) = wipe(&paths, &hub, &account).await {
                eprintln!("syncd: cannot wipe account {account}: {why}");
            }
        }
    });
    Ok(())
}

/// Makes accountd know this connection: the call itself is the introduction, and a failure (no
/// accountd yet) is fine, the next owner change tries again.
async fn introduce(connection: &Connection) {
    if let Ok(grants) = GrantsProxy::new(connection).await {
        let _ = grants.list().await;
    }
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

//! [`ProcCallers`]: the callers of a bus connection, by the process behind each sender.

use super::standing::caller_of;
use super::translate::{manager_need, peephole_table};
use super::{Caller, CallerTable, Callers};
use crate::BusConnection;
use peephole_bus::{BusPeers, ManagerCheck};
use peephole_core::Standing;

/// Callers by process: peephole holds the process behind a sender (a pidfd), reads what `/proc`
/// and the systemd manager say of it, and judges it against the table's service rows; the table
/// then names the role. The table is porter's own (`callers.toml`), translated once when this is
/// built (see `translate`).
#[derive(Debug)]
pub struct ProcCallers {
    table: CallerTable,
    peers: Peers,
}

/// What looks at senders: the machine's `/proc` and the user manager on the connection's bus, or
/// (only in a test build) a directory standing for both.
#[derive(Debug)]
pub(super) enum Peers {
    /// The real thing.
    System(BusPeers),
    /// A fixture tree ([`super::fixture`]).
    #[cfg(feature = "test-proc-root")]
    Tree(BusPeers<super::fixture::DirProc, super::fixture::DirUnits>),
}

impl Peers {
    /// Boxed, so a daemon future that awaits a caller does not carry peephole's whole lookup.
    fn who<'a>(
        &'a self,
        sender: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Standing>> + Send + 'a>> {
        match self {
            Self::System(peers) => Box::pin(async move { peers.who(sender).await.ok() }),
            #[cfg(feature = "test-proc-root")]
            Self::Tree(peers) => Box::pin(async move { peers.who(sender).await.ok() }),
        }
    }
}

impl ProcCallers {
    /// Resolves senders on `connection` through `table`, reading the system's `/proc` and asking
    /// the user manager on the same bus for a service's unit. The manager is believed only while
    /// its own process is the system's `systemd`.
    pub fn new(connection: BusConnection, table: CallerTable) -> Self {
        let rows = peephole_table(&table);
        let peers = BusPeers::new(connection, rows, ManagerCheck::Verified(manager_need()));
        Self {
            table,
            peers: Peers::System(peers),
        }
    }

    /// Builds callers over `peers` (the fixture constructors' way in).
    #[cfg(feature = "test-proc-root")]
    pub(super) fn over(table: CallerTable, peers: Peers) -> Self {
        Self { table, peers }
    }
}

impl Callers for ProcCallers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        let standing = self.peers.who(sender).await?;
        caller_of(&standing, &self.table)
    }
}

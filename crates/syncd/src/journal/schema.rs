//! The journal file's schema and its migrations. `MIGRATIONS[n]` takes version `n` to `n + 1`;
//! the applied versions are rows of `schema_migrations`, so a file written by an older syncd
//! opens, is brought forward in one transaction per step, and a file from a newer one is refused.

use super::JournalError;
use rusqlite::{Connection, Transaction};

/// Version 1: items, anchors, tombstones, conflicts (design/31 §6.1 "Journal").
const V1: &str = "
CREATE TABLE items (
    local_id        TEXT PRIMARY KEY NOT NULL,
    remote_id       TEXT UNIQUE,
    path            TEXT NOT NULL,
    size            INTEGER NOT NULL,
    hash            TEXT,
    remote_version  TEXT,
    base_version    TEXT,
    state           TEXT NOT NULL
);
CREATE TABLE anchors (
    id          INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    anchor      TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE tombstones (
    remote_id   TEXT PRIMARY KEY NOT NULL,
    version     TEXT NOT NULL,
    deleted_at  INTEGER NOT NULL,
    origin      TEXT NOT NULL,
    ack         TEXT NOT NULL
);
CREATE TABLE conflicts (
    number      INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    item        TEXT NOT NULL,
    base        TEXT,
    remote      TEXT NOT NULL,
    local_id    TEXT NOT NULL,
    local_hash  TEXT,
    at          INTEGER NOT NULL
);
";

/// Every migration, oldest first.
pub(super) const MIGRATIONS: &[&str] = &[V1];

/// The schema version this build writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

/// Brings `connection` to the last of `migrations`: creates the migration table, applies what is
/// missing. A file ahead of `migrations` is an error naming its version.
pub(super) fn migrate(
    connection: &mut Connection,
    migrations: &[&str],
) -> Result<(), JournalError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY NOT NULL,
            applied_at  INTEGER NOT NULL
        );",
    )?;
    let have: i64 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    let want = i64::try_from(migrations.len()).unwrap_or(i64::MAX);
    if have > want {
        return Err(JournalError::Newer { found: have });
    }
    for (index, sql) in migrations
        .iter()
        .enumerate()
        .skip(usize::try_from(have).unwrap_or(0))
    {
        let tx = connection.transaction()?;
        apply(&tx, sql, index + 1)?;
        tx.commit()?;
    }
    Ok(())
}

fn apply(tx: &Transaction<'_>, sql: &str, version: usize) -> rusqlite::Result<()> {
    tx.execute_batch(sql)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, strftime('%s','now'))",
        [i64::try_from(version).unwrap_or(i64::MAX)],
    )?;
    Ok(())
}

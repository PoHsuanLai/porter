//! Rows to values and back: the one place SQL text lives.

use super::{JournalError, Op};
use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    Acknowledgement, Anchor, BaseVersion, Conflict, ContentHash, ItemPath, ItemState, JournalItem,
    LocalId, RemoteId, RemoteSide, RemoteVersion, StoredAnchor, StoredConflict, StoredTombstone,
    Tombstone, TombstoneOrigin,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

fn corrupt(what: &str, value: &str) -> JournalError {
    JournalError::Corrupt(format!("{what} `{value}` is not one this syncd knows"))
}

fn slug_of<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_slug<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, JournalError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|_| corrupt(what, text))
}

fn base_text(base: &BaseVersion) -> Option<&str> {
    match base {
        BaseVersion::Absent => None,
        BaseVersion::At(RemoteVersion(v)) => Some(v),
    }
}

fn base_of(text: Option<String>) -> BaseVersion {
    text.map_or(BaseVersion::Absent, |v| BaseVersion::At(RemoteVersion(v)))
}

fn size_of(bytes: Bytes) -> i64 {
    i64::try_from(bytes.0).unwrap_or(i64::MAX)
}

pub(super) fn apply(tx: &Transaction<'_>, op: &Op) -> Result<(), JournalError> {
    match op {
        Op::PutItem(item) => put_item(tx, item)?,
        Op::DeleteItem(local) => {
            tx.execute("DELETE FROM items WHERE local_id = ?1", [&local.0])?;
        }
        Op::SetAnchor(StoredAnchor { anchor, at }) => {
            tx.execute(
                "INSERT INTO anchors (id, anchor, updated_at) VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET anchor = ?1, updated_at = ?2",
                params![anchor.0, at.0],
            )?;
        }
        Op::ClearAnchor => {
            tx.execute("DELETE FROM anchors", [])?;
        }
        Op::PutTombstone(stored) => {
            tx.execute(
                "INSERT OR REPLACE INTO tombstones (remote_id, version, deleted_at, origin, ack)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    stored.tombstone.id.0,
                    stored.tombstone.version.0,
                    stored.tombstone.deleted_at.0,
                    slug_of(&stored.origin),
                    slug_of(&stored.ack)
                ],
            )?;
        }
        Op::Acknowledge(id) => {
            tx.execute(
                "UPDATE tombstones SET ack = ?2 WHERE remote_id = ?1",
                params![id.0, slug_of(&Acknowledgement::Acknowledged)],
            )?;
        }
        Op::CompactTombstones => {
            tx.execute(
                "DELETE FROM tombstones WHERE ack = ?1",
                [slug_of(&Acknowledgement::Acknowledged)],
            )?;
        }
        Op::AddConflict(stored) => {
            let remote = serde_json::to_string(&stored.conflict.remote)
                .map_err(|e| JournalError::Corrupt(e.to_string()))?;
            tx.execute(
                "INSERT INTO conflicts (item, base, remote, local_id, local_hash, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    stored.conflict.item.0,
                    base_text(&stored.conflict.base),
                    remote,
                    stored.local.0,
                    stored.local_hash.as_ref().map(|h| h.0.as_str()),
                    stored.at.0
                ],
            )?;
        }
        Op::DropConflict(number) => {
            tx.execute("DELETE FROM conflicts WHERE number = ?1", [number])?;
        }
    }
    Ok(())
}

fn put_item(tx: &Transaction<'_>, item: &JournalItem) -> Result<(), JournalError> {
    tx.execute(
        "INSERT INTO items (local_id, remote_id, path, size, hash, remote_version, base_version, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(local_id) DO UPDATE SET remote_id = ?2, path = ?3, size = ?4, hash = ?5,
             remote_version = ?6, base_version = ?7, state = ?8",
        params![
            item.local.0,
            item.remote.as_ref().map(|r| r.0.as_str()),
            item.path.0,
            size_of(item.size),
            item.hash.as_ref().map(|h| h.0.as_str()),
            item.remote_version.as_ref().map(|v| v.0.as_str()),
            base_text(&item.base),
            item.state.slug()
        ],
    )?;
    Ok(())
}

fn item_of(row: &Row<'_>) -> rusqlite::Result<Result<JournalItem, JournalError>> {
    let state: String = row.get("state")?;
    let size: i64 = row.get("size")?;
    Ok(ItemState::from_slug(&state)
        .ok_or_else(|| corrupt("item state", &state))
        .and_then(|state| {
            Ok(JournalItem {
                local: LocalId(row.get("local_id")?),
                remote: row.get::<_, Option<String>>("remote_id")?.map(RemoteId),
                path: ItemPath(row.get("path")?),
                size: Bytes(u64::try_from(size).unwrap_or(0)),
                hash: row.get::<_, Option<String>>("hash")?.map(ContentHash),
                remote_version: row
                    .get::<_, Option<String>>("remote_version")?
                    .map(RemoteVersion),
                base: base_of(row.get("base_version")?),
                state,
            })
        }))
}

pub(super) fn items(connection: &Connection) -> Result<Vec<JournalItem>, JournalError> {
    let mut statement = connection.prepare("SELECT * FROM items ORDER BY local_id")?;
    let rows = statement.query_map([], item_of)?;
    rows.map(|row| row?).collect()
}

pub(super) fn item_where(
    connection: &Connection,
    clause: &str,
    value: &str,
) -> Result<Option<JournalItem>, JournalError> {
    let sql = format!("SELECT * FROM items WHERE {clause}");
    connection
        .query_row(&sql, [value], item_of)
        .optional()?
        .transpose()
}

pub(super) fn anchor(connection: &Connection) -> Result<Option<StoredAnchor>, JournalError> {
    Ok(connection
        .query_row(
            "SELECT anchor, updated_at FROM anchors WHERE id = 1",
            [],
            |row| {
                Ok(StoredAnchor {
                    anchor: Anchor(row.get(0)?),
                    at: UnixSeconds(row.get(1)?),
                })
            },
        )
        .optional()?)
}

pub(super) fn tombstones(connection: &Connection) -> Result<Vec<StoredTombstone>, JournalError> {
    let mut statement = connection.prepare("SELECT * FROM tombstones ORDER BY remote_id")?;
    let rows = statement.query_map([], |row| {
        let (origin, ack): (String, String) = (row.get("origin")?, row.get("ack")?);
        let tombstone = Tombstone {
            id: RemoteId(row.get("remote_id")?),
            version: RemoteVersion(row.get("version")?),
            deleted_at: UnixSeconds(row.get("deleted_at")?),
        };
        Ok((tombstone, origin, ack))
    })?;
    rows.map(|row| {
        let (tombstone, origin, ack) = row?;
        Ok(StoredTombstone {
            tombstone,
            origin: from_slug::<TombstoneOrigin>("tombstone origin", &origin)?,
            ack: from_slug::<Acknowledgement>("tombstone ack", &ack)?,
        })
    })
    .collect()
}

pub(super) fn conflicts(connection: &Connection) -> Result<Vec<StoredConflict>, JournalError> {
    let mut statement = connection.prepare("SELECT * FROM conflicts ORDER BY number")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>("number")?,
            row.get::<_, String>("item")?,
            row.get::<_, Option<String>>("base")?,
            row.get::<_, String>("remote")?,
            row.get::<_, String>("local_id")?,
            row.get::<_, Option<String>>("local_hash")?,
            row.get::<_, i64>("at")?,
        ))
    })?;
    rows.map(|row| {
        let (number, item, base, remote, local, local_hash, at) = row?;
        let remote: RemoteSide =
            serde_json::from_str(&remote).map_err(|_| corrupt("conflict side", &remote))?;
        Ok(StoredConflict {
            number: Some(number),
            conflict: Conflict {
                item: RemoteId(item),
                base: base_of(base),
                remote,
            },
            local: LocalId(local),
            local_hash: local_hash.map(ContentHash),
            at: UnixSeconds(at),
        })
    })
    .collect()
}

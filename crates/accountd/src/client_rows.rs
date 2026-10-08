//! What Settings changes in the person's `clients.toml`: one row's client id or secret, with the
//! rest of the file as it was. The row's other lines (Google's `testing` and `byo`, which
//! `porter_oauth` reads beside the row, and any `endpoints`) and every other row are kept, so a
//! hand-written Google row survives its key being changed in Settings.

use porter_provider::{ClientChannel, Issuer, parse_clients};
use toml::{Table, Value};

/// What is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowEdit<'a> {
    /// The client id; empty removes the row (its secret with it).
    Id(&'a str),
    /// The application secret; empty removes it from the row.
    Secret(&'a str),
}

/// Why the edit was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowRefused {
    /// A secret for an issuer with no row: a row needs its client id first.
    NoKey,
    /// The result would not be a valid clients file.
    Unwritable(String),
}

fn slug<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `text` (the file as it is; empty or damaged reads as no rows) with `edit` made to the row of
/// `issuer` on `channel`. A new row for Google says `testing = true`: a person's own Google
/// client is in testing until Google verifies it, which is what makes porter say why Google
/// signs the account out after seven days.
pub(crate) fn edited(
    text: &str,
    issuer: Issuer,
    channel: ClientChannel,
    edit: RowEdit<'_>,
) -> Result<String, RowRefused> {
    let mut doc: Table = toml::from_str(text).unwrap_or_default();
    let (issuer_slug, channel_slug) = (slug(&issuer), slug(&channel));
    let mut rows: Vec<Value> = match doc.remove("client") {
        Some(Value::Array(rows)) => rows,
        _ => Vec::new(),
    };
    let at = rows.iter().position(|row| {
        row.get("issuer").and_then(Value::as_str) == Some(issuer_slug.as_str())
            && row.get("channel").and_then(Value::as_str) == Some(channel_slug.as_str())
    });
    match (edit, at) {
        (RowEdit::Id(""), Some(at)) => {
            rows.remove(at);
        }
        (RowEdit::Id(""), None) | (RowEdit::Secret(""), None) => {}
        (RowEdit::Id(id), Some(at)) => set(&mut rows[at], "client_id", id),
        (RowEdit::Id(id), None) => {
            let mut row = Table::new();
            row.insert("issuer".into(), Value::String(issuer_slug.clone()));
            row.insert("channel".into(), Value::String(channel_slug.clone()));
            row.insert("client_id".into(), Value::String(id.to_owned()));
            if issuer == Issuer::Google {
                row.insert("testing".into(), Value::Boolean(true));
            }
            rows.push(Value::Table(row));
        }
        (RowEdit::Secret(""), Some(at)) => {
            if let Some(row) = rows[at].as_table_mut() {
                row.remove("client_secret");
            }
        }
        (RowEdit::Secret(secret), Some(at)) => set(&mut rows[at], "client_secret", secret),
        (RowEdit::Secret(_), None) => return Err(RowRefused::NoKey),
    }
    if !rows.is_empty() {
        doc.insert("client".into(), Value::Array(rows));
    }
    let out = toml::to_string(&doc).map_err(|e| RowRefused::Unwritable(e.to_string()))?;
    parse_clients(&out).map_err(|e| RowRefused::Unwritable(e.to_string()))?;
    Ok(out)
}

fn set(row: &mut Value, field: &str, value: &str) {
    if let Some(row) = row.as_table_mut() {
        row.insert(field.into(), Value::String(value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAND_WRITTEN: &str = r#"
[[client]]
issuer = "google"
channel = "development"
client_id = "old.apps.googleusercontent.com"
client_secret = "GOCSPX-old"
testing = true
byo = true

[[client]]
issuer = "microsoft"
channel = "development"
client_id = "ms-mine"
"#;

    fn row(text: &str, issuer: &str) -> Option<Table> {
        let doc: Table = toml::from_str(text).expect("toml");
        doc.get("client")?
            .as_array()?
            .iter()
            .find(|r| r.get("issuer").and_then(Value::as_str) == Some(issuer))
            .and_then(Value::as_table)
            .cloned()
    }

    #[test]
    fn a_new_key_changes_only_the_id_and_keeps_the_rows_other_lines_and_rows() {
        let out = edited(
            HAND_WRITTEN,
            Issuer::Google,
            ClientChannel::Development,
            RowEdit::Id("new.apps.googleusercontent.com"),
        )
        .expect("edited");
        let google = row(&out, "google").expect("google row");
        assert_eq!(
            google["client_id"].as_str(),
            Some("new.apps.googleusercontent.com")
        );
        assert_eq!(google["client_secret"].as_str(), Some("GOCSPX-old"));
        assert_eq!(google["testing"].as_bool(), Some(true));
        assert_eq!(google["byo"].as_bool(), Some(true));
        assert_eq!(
            row(&out, "microsoft").expect("kept")["client_id"].as_str(),
            Some("ms-mine")
        );
    }

    #[test]
    fn a_google_key_set_in_an_empty_file_is_a_testing_row_and_takes_its_secret() {
        let out = edited(
            "",
            Issuer::Google,
            ClientChannel::Stable,
            RowEdit::Id("mine.apps.googleusercontent.com"),
        )
        .expect("edited");
        let out = edited(
            &out,
            Issuer::Google,
            ClientChannel::Stable,
            RowEdit::Secret("GOCSPX-x"),
        )
        .expect("secret");
        let file = parse_clients(&out).expect("valid");
        assert_eq!(file.clients.len(), 1);
        assert_eq!(
            file.clients[0]
                .client_secret
                .as_ref()
                .map(|s| s.expose().to_owned()),
            Some("GOCSPX-x".to_owned())
        );
        assert_eq!(
            row(&out, "google").expect("row")["testing"].as_bool(),
            Some(true)
        );
        // Microsoft's new row says nothing of testing.
        let ms = edited(
            "",
            Issuer::Microsoft,
            ClientChannel::Stable,
            RowEdit::Id("ms"),
        )
        .expect("edited");
        assert_eq!(row(&ms, "microsoft").expect("row").get("testing"), None);
    }

    #[test]
    fn a_secret_needs_a_key_first_and_clearing_the_key_takes_the_row() {
        assert_eq!(
            edited(
                "",
                Issuer::Google,
                ClientChannel::Stable,
                RowEdit::Secret("GOCSPX-x")
            ),
            Err(RowRefused::NoKey)
        );
        let out = edited(
            HAND_WRITTEN,
            Issuer::Google,
            ClientChannel::Development,
            RowEdit::Id(""),
        )
        .expect("cleared");
        assert_eq!(row(&out, "google"), None);
        assert!(row(&out, "microsoft").is_some());
        let out = edited(
            HAND_WRITTEN,
            Issuer::Google,
            ClientChannel::Development,
            RowEdit::Secret(""),
        )
        .expect("secret cleared");
        assert_eq!(row(&out, "google").expect("row").get("client_secret"), None);
    }

    #[test]
    fn a_damaged_file_is_replaced_by_the_one_row() {
        let out = edited(
            "[[client",
            Issuer::Microsoft,
            ClientChannel::Stable,
            RowEdit::Id("ms"),
        )
        .expect("edited");
        assert_eq!(parse_clients(&out).expect("valid").clients.len(), 1);
    }
}

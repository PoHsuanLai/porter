//! porter's caller table as peephole's [`Table`], and the names between the two.
//!
//! peephole knows a service by unit and program; porter's rows say app, unit and role. The
//! smallest sound translation, and the only one made:
//!
//! - a row with a `unit` and at least one program ([`CallerRow::images`]) becomes a `[[peer]]` row
//!   of the same app, unit and role, with those programs as its `exe` patterns. A unit with no
//!   program is left out: the unit's name alone proves nothing, so its process is not a service
//!   and so is nobody;
//! - a row with no unit is not translated: it names an app by the scope or sandbox it runs in,
//!   and [`CallerTable::role_of`] gives its role;
//! - one row is added for the unit manager itself (`systemd`'s image), the role the bus crate
//!   demands of whoever answers questions about units ([`manager_need`]);
//! - the role is porter's own spelling (`porter_daemon`, `sheet_host`) so that nothing is
//!   renamed in either direction;
//! - the first row for a unit wins, as in [`CallerTable::resolve_unit`].

use super::{CallerRole, CallerRow, CallerTable};
use peephole_core::{Proof, Requirement, RoleName, Table};

/// The role of the unit manager's row. No porter role has this name.
const MANAGER_ROLE: &str = "manager";

/// The row for the unit manager: `systemd --user` runs the system's `systemd`.
const MANAGER_ROW: &str = r#"
[[peer]]
app  = "org.freedesktop.systemd1"
exe  = ["/usr/lib/systemd/systemd", "/lib/systemd/systemd"]
role = "manager"
"#;

/// What the owner of the unit manager's bus name must be before its answers about units are
/// believed: the manager row, at the proof an image alone gives.
pub(super) fn manager_need() -> Requirement {
    Requirement::new(RoleName::from_static(MANAGER_ROLE), Proof::Exe)
}

/// The role `role` as peephole spells it.
pub(super) fn role_name(role: CallerRole) -> Option<RoleName> {
    let value = serde_json::to_value(role).ok()?;
    RoleName::new(value.as_str()?).ok()
}

/// The role a peephole role name stands for; `None` for a name no porter role has.
pub(super) fn role_of_name(name: &RoleName) -> Option<CallerRole> {
    serde_json::from_value(serde_json::Value::String(name.as_str().to_owned())).ok()
}

/// A string as a TOML basic string (JSON's escapes are TOML's).
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The `[[peer]]` text of `row`, or `None` when it is not a service peephole can judge.
fn peer_text(row: &CallerRow) -> Option<String> {
    let unit = row.unit.as_deref()?;
    let images = row.images();
    if images.is_empty() {
        return None;
    }
    let role = role_name(row.role)?;
    let exe: Vec<String> = images.iter().map(|image| quoted(image)).collect();
    Some(format!(
        "[[peer]]\napp = {}\nunit = {}\nexe = [{}]\nrole = {}\n",
        quoted(row.app.as_str()),
        quoted(unit),
        exe.join(", "),
        quoted(role.as_str()),
    ))
}

/// One row as a table; a row peephole refuses (a program path it does not take) is left out with
/// one line on stderr, as [`CallerTable::layered`] does for a user row, and the rest stand.
fn one_row(row: &CallerRow) -> Option<Table> {
    let text = peer_text(row)?;
    match Table::from_toml(&text) {
        Ok(table) => Some(table),
        Err(why) => {
            eprintln!(
                "porter: callers: ignoring the row for {}: {why}",
                row.unit.as_deref().unwrap_or(row.app.as_str())
            );
            None
        }
    }
}

/// `table` as the table peephole judges against, with the unit manager's row.
pub(super) fn peephole_table(table: &CallerTable) -> Table {
    let manager = Table::from_toml(MANAGER_ROW).unwrap_or_default();
    table
        .callers
        .iter()
        .filter_map(one_row)
        .fold(Table::empty(), Table::merge)
        .merge(manager)
}

#[cfg(test)]
mod tests;

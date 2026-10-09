//! The D-Bus shape of a machine (`(sa{sv})`): its Tailscale node id, then a vardict of its
//! fields by name. The lists are real string arrays (`as`), not the lists of variants that
//! `json_value` writes, since a client reads them as they are.

use crate::args::Details;
use crate::json_value::bad;
use porter_core::{CoreError, Machine, MachineOwner, NodeId, UnixSeconds};
use std::net::IpAddr;
use zbus::zvariant::{OwnedValue, Value};

/// The vardict key of the node id (`s`, the same as the row's first part).
pub const MACHINE_KEY_NODE: &str = "node";
/// The vardict key of the name people call the computer by (`s`).
pub const MACHINE_KEY_NAME: &str = "name";
/// The vardict key of its MagicDNS name without the dot at the end (`s`).
pub const MACHINE_KEY_DNS: &str = "dns";
/// The vardict key of its Tailscale addresses (`as`).
pub const MACHINE_KEY_ADDRESSES: &str = "addresses";
/// The vardict key of its operating system (`s`).
pub const MACHINE_KEY_OS: &str = "os";
/// The vardict key of whether it is connected (`b`).
pub const MACHINE_KEY_ONLINE: &str = "online";
/// The vardict key of when it was last connected (`x`, Unix seconds): absent while it is online
/// and when Tailscale does not know.
pub const MACHINE_KEY_LAST_SEEN: &str = "last_seen";
/// The vardict key of whether Tailscale's own SSH is on (`b`).
pub const MACHINE_KEY_SSH: &str = "ssh";
/// The vardict key of the SSH host keys it advertises (`as`).
pub const MACHINE_KEY_SSH_HOST_KEYS: &str = "ssh_host_keys";
/// The vardict key of whose it is (`s`: `mine`, `shared` or `tagged`).
pub const MACHINE_KEY_OWNER: &str = "owner";

fn owned(value: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(value).ok()
}

fn strings(items: impl IntoIterator<Item = String>) -> Value<'static> {
    Value::Array(items.into_iter().collect::<Vec<String>>().into())
}

/// A machine as its node id and a vardict of the rest.
pub fn machine_to_dbus(machine: &Machine) -> (String, Details) {
    let mut fields: Vec<(&str, Option<OwnedValue>)> = vec![
        (
            MACHINE_KEY_NODE,
            owned(Value::from(machine.node.as_str().to_owned())),
        ),
        (MACHINE_KEY_NAME, owned(Value::from(machine.name.clone()))),
        (MACHINE_KEY_DNS, owned(Value::from(machine.dns.clone()))),
        (
            MACHINE_KEY_ADDRESSES,
            owned(strings(machine.addresses.iter().map(IpAddr::to_string))),
        ),
        (MACHINE_KEY_OS, owned(Value::from(machine.os.clone()))),
        (MACHINE_KEY_ONLINE, owned(Value::from(machine.online))),
        (MACHINE_KEY_SSH, owned(Value::from(machine.ssh))),
        (
            MACHINE_KEY_SSH_HOST_KEYS,
            owned(strings(machine.ssh_host_keys.iter().cloned())),
        ),
        (
            MACHINE_KEY_OWNER,
            owned(Value::from(machine.owner.slug().to_owned())),
        ),
    ];
    if let Some(at) = machine.last_seen {
        fields.push((MACHINE_KEY_LAST_SEEN, owned(Value::from(at.0))));
    }
    let details = fields
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect();
    (machine.node.as_str().to_owned(), details)
}

fn get<T: TryFrom<OwnedValue>>(details: &Details, key: &str) -> Option<T> {
    T::try_from(details.get(key)?.try_clone().ok()?).ok()
}

/// The machine a row carries, or why it is not one. A key a newer accountd adds is ignored; the
/// name, the network name and the owner must be there.
pub fn machine_from_dbus(row: &(String, Details)) -> Result<Machine, CoreError> {
    let (node, details) = row;
    let text = |key: &str| get::<String>(details, key);
    let list = |key: &str| get::<Vec<String>>(details, key).unwrap_or_default();
    let owner = text(MACHINE_KEY_OWNER)
        .and_then(|slug| MachineOwner::from_slug(&slug))
        .ok_or_else(|| bad("machine: no owner"))?;
    Ok(Machine {
        node: NodeId::parse(node)?,
        name: text(MACHINE_KEY_NAME).ok_or_else(|| bad("machine: no name"))?,
        dns: text(MACHINE_KEY_DNS).ok_or_else(|| bad("machine: no network name"))?,
        addresses: list(MACHINE_KEY_ADDRESSES)
            .iter()
            .filter_map(|address| address.parse().ok())
            .collect(),
        os: text(MACHINE_KEY_OS).unwrap_or_default(),
        online: get::<bool>(details, MACHINE_KEY_ONLINE).unwrap_or(false),
        last_seen: get::<i64>(details, MACHINE_KEY_LAST_SEEN).map(UnixSeconds),
        ssh: get::<bool>(details, MACHINE_KEY_SSH).unwrap_or(false),
        ssh_host_keys: list(MACHINE_KEY_SSH_HOST_KEYS),
        owner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine {
            node: NodeId::parse("nPXS7a2CNTRL").expect("id"),
            name: "pi".into(),
            dns: "pi.tail1234.ts.net".into(),
            addresses: vec![
                "100.64.0.2".parse().expect("ip"),
                "fd7a:115c:a1e0::2".parse().expect("ip"),
            ],
            os: "linux".into(),
            online: false,
            last_seen: Some(UnixSeconds(1_790_000_000)),
            ssh: true,
            ssh_host_keys: vec!["ssh-ed25519 AAAA".into()],
            owner: MachineOwner::Tagged,
        }
    }

    #[test]
    fn a_machine_survives_the_bus_shape() {
        let row = machine_to_dbus(&machine());
        assert_eq!(row.0, "nPXS7a2CNTRL");
        assert_eq!(machine_from_dbus(&row), Ok(machine()));
    }

    #[test]
    fn the_lists_are_string_arrays_and_last_seen_is_absent_while_online() {
        let mut online = machine();
        online.online = true;
        online.last_seen = None;
        let (_, details) = machine_to_dbus(&online);
        assert!(!details.contains_key(MACHINE_KEY_LAST_SEEN));
        for key in [MACHINE_KEY_ADDRESSES, MACHINE_KEY_SSH_HOST_KEYS] {
            let value = details.get(key).expect(key);
            assert_eq!(value.value_signature().to_string(), "as", "{key}");
        }
    }

    #[test]
    fn a_row_missing_what_a_machine_needs_is_refused() {
        let (node, details) = machine_to_dbus(&machine());
        for key in [MACHINE_KEY_NAME, MACHINE_KEY_DNS, MACHINE_KEY_OWNER] {
            let mut cut = details.clone();
            cut.remove(key);
            assert!(machine_from_dbus(&(node.clone(), cut)).is_err(), "{key}");
        }
        assert!(machine_from_dbus(&("not a node".to_owned(), details)).is_err());
    }
}

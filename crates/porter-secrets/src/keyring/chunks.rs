//! Attribution: ported from `mail-runtime/src/secrets/chunks.rs` in mailo (MIT OR Apache-2.0, same
//! author); the error type and the marker are porter's.
//!
//! One credential over several keyring entries, where a platform's entries are too small for it.
//!
//! The Windows Credential Manager holds at most 2560 bytes in an entry, and a password is kept
//! there as UTF-16, so 1280 code units. A Microsoft sign-in's access token alone is a JWT of one
//! to two thousand characters, and the stored credential is that, its refresh token and its
//! expiry as JSON; an S/MIME key in PEM is longer still. Stored whole, `put` would fail with
//! the store's `TooLong` and the account could never be signed in.
//!
//! So a value over the platform's [`Limit`] is cut into parts, each in an entry of its own named
//! `<name>#1`, `<name>#2` and so on, and the entry `<name>` holds only a marker saying how many
//! there are. A value that fits is stored whole in `<name>` as before, which is every value on a
//! platform with no limit, so the Keychain hold exactly what they held.
//! Every value this module is given is JSON (`encode`), which begins with `{` or `"`, so a
//! stored value can never be mistaken for the marker.

use crate::error::SecretsError;

/// How long one entry's value may be on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Any length: the Keychain.
    #[cfg_attr(windows, allow(dead_code))]
    None,
    /// At most this many UTF-16 code units: what the Windows Credential Manager holds.
    Utf16Units(usize),
}

/// The platform's own entries, by name. The keyring in the program, a map with a limit in the
/// tests.
pub trait Slots {
    /// The value in `name`, or `None` when there is no such entry.
    fn read(&self, name: &str) -> Result<Option<String>, SecretsError>;
    fn write(&self, name: &str, value: &str) -> Result<(), SecretsError>;
    /// Remove `name`. An entry that is already gone is not an error.
    fn remove(&self, name: &str) -> Result<(), SecretsError>;
}

/// What the head entry of a value in parts holds, before the count.
const MARK: &str = "porter-parts:";

/// More parts than this is a damaged head entry, not a credential: sixty-four parts are some
/// eighty thousand characters on Windows.
const MOST_PARTS: usize = 64;

fn part_name(name: &str, index: usize) -> String {
    format!("{name}#{index}")
}

/// Store `value` under `name`, in parts when it does not fit in one entry.
///
/// Parts left over from a longer value stored earlier are removed once the head names the new
/// count. A write that fails part-way can leave parts of both values; the JSON they make then
/// does not parse, which the store reports as a credential to enter again, as it would a
/// missing one.
pub fn put(slots: &dyn Slots, name: &str, value: &str, limit: Limit) -> Result<(), SecretsError> {
    let before = count(slots, name)?;
    let parts = pieces(value, limit);
    let now = match parts.as_slice() {
        [whole] => {
            slots.write(name, whole)?;
            0
        }
        parts => {
            for (index, part) in parts.iter().enumerate() {
                slots.write(&part_name(name, index + 1), part)?;
            }
            slots.write(name, &format!("{MARK}{}", parts.len()))?;
            parts.len()
        }
    };
    for index in now + 1..=before {
        slots.remove(&part_name(name, index))?;
    }
    Ok(())
}

/// The value under `name`, put back together; `None` when there is none.
pub fn get(slots: &dyn Slots, name: &str) -> Result<Option<String>, SecretsError> {
    let Some(head) = slots.read(name)? else {
        return Ok(None);
    };
    let Some(parts) = parts_in(&head)? else {
        return Ok(Some(head));
    };
    let mut value = String::new();
    for index in 1..=parts {
        match slots.read(&part_name(name, index))? {
            Some(part) => value.push_str(&part),
            None => {
                return Err(SecretsError::Unreadable);
            }
        }
    }
    Ok(Some(value))
}

/// Remove `name` and every part of it.
pub fn forget(slots: &dyn Slots, name: &str) -> Result<(), SecretsError> {
    // A head that cannot be read still has to go, and so do the parts it may have had.
    let parts = count(slots, name).unwrap_or(MOST_PARTS);
    for index in 1..=parts {
        slots.remove(&part_name(name, index))?;
    }
    slots.remove(name)
}

/// How many parts the value under `name` is stored in: 0 for none or a value stored whole.
fn count(slots: &dyn Slots, name: &str) -> Result<usize, SecretsError> {
    match slots.read(name)? {
        Some(head) => Ok(parts_in(&head)?.unwrap_or(0)),
        None => Ok(0),
    }
}

/// The part count a head entry names, `None` for a value stored whole.
fn parts_in(head: &str) -> Result<Option<usize>, SecretsError> {
    let Some(count) = head.strip_prefix(MARK) else {
        return Ok(None);
    };
    match count.parse::<usize>() {
        Ok(count) if (2..=MOST_PARTS).contains(&count) => Ok(Some(count)),
        _ => Err(SecretsError::Unreadable),
    }
}

/// `value` cut so that every piece fits `limit`, never inside a character.
pub fn pieces(value: &str, limit: Limit) -> Vec<&str> {
    let Limit::Utf16Units(most) = limit else {
        return vec![value];
    };
    // A character is at most two units, so a limit under two could never place one.
    let most = most.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (at, c) in value.char_indices() {
        if units + c.len_utf16() > most {
            out.push(&value[start..at]);
            start = at;
            units = 0;
        }
        units += c.len_utf16();
    }
    out.push(&value[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A keyring whose entries refuse anything over `limit`, as the Credential Manager does.
    #[derive(Default)]
    struct Small {
        limit: usize,
        entries: Mutex<BTreeMap<String, String>>,
    }

    impl Small {
        fn with_limit(limit: usize) -> Self {
            Small {
                limit,
                ..Small::default()
            }
        }

        fn names(&self) -> Vec<String> {
            self.entries.lock().unwrap().keys().cloned().collect()
        }
    }

    impl Slots for Small {
        fn read(&self, name: &str) -> Result<Option<String>, SecretsError> {
            Ok(self.entries.lock().unwrap().get(name).cloned())
        }

        fn write(&self, name: &str, value: &str) -> Result<(), SecretsError> {
            if value.encode_utf16().count() > self.limit {
                return Err(SecretsError::Unreadable);
            }
            self.entries
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.to_owned());
            Ok(())
        }

        fn remove(&self, name: &str) -> Result<(), SecretsError> {
            self.entries.lock().unwrap().remove(name);
            Ok(())
        }
    }

    /// Room for the head's marker and count, and small enough that a test value needs parts.
    const LIMIT: Limit = Limit::Utf16Units(16);
    const EIGHT: Limit = Limit::Utf16Units(8);

    #[test]
    fn a_value_is_cut_where_it_fits_and_never_inside_a_character() {
        let cases: &[(&str, Limit, &[&str])] = &[
            ("{\"a\":1}", Limit::None, &["{\"a\":1}"]),
            ("{\"a\":1}", EIGHT, &["{\"a\":1}"]),
            ("0123456789", EIGHT, &["01234567", "89"]),
            // Each of these is two UTF-16 units, as it is to Windows.
            ("😀😀😀😀😀", EIGHT, &["😀😀😀😀", "😀"]),
            ("1234567😀", EIGHT, &["1234567", "😀"]),
            ("", EIGHT, &[""]),
        ];
        for (value, limit, expect) in cases {
            assert_eq!(pieces(value, *limit), *expect, "{value:?} at {limit:?}");
            for piece in pieces(value, *limit) {
                if let Limit::Utf16Units(most) = limit {
                    assert!(piece.encode_utf16().count() <= *most, "{piece:?}");
                }
            }
        }
    }

    #[test]
    fn a_value_too_long_for_one_entry_is_kept_in_parts_and_read_back_whole() {
        let slots = Small::with_limit(16);
        let token = "{\"access\":\"eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9\"}";
        put(&slots, "acct:oauth", token, LIMIT).unwrap();
        assert!(slots.names().len() > 2, "{:?}", slots.names());
        assert_eq!(get(&slots, "acct:oauth").unwrap().as_deref(), Some(token));
    }

    #[test]
    fn a_value_that_fits_is_one_entry_holding_exactly_the_value() {
        // What the Keychain have always held: nothing about them moves.
        let slots = Small::with_limit(usize::MAX);
        put(&slots, "acct:incoming", "\"hunter2\"", Limit::None).unwrap();
        assert_eq!(slots.names(), ["acct:incoming"]);
        assert_eq!(
            slots.read("acct:incoming").unwrap().as_deref(),
            Some("\"hunter2\"")
        );
    }

    #[test]
    fn a_shorter_value_leaves_no_parts_of_the_longer_one_behind() {
        let slots = Small::with_limit(16);
        put(&slots, "k", &"x".repeat(40), LIMIT).unwrap();
        put(&slots, "k", &"y".repeat(20), LIMIT).unwrap();
        assert_eq!(slots.names(), ["k", "k#1", "k#2"]);
        assert_eq!(get(&slots, "k").unwrap(), Some("y".repeat(20)));
        put(&slots, "k", "\"z\"", LIMIT).unwrap();
        assert_eq!(slots.names(), ["k"]);
        assert_eq!(get(&slots, "k").unwrap().as_deref(), Some("\"z\""));
    }

    #[test]
    fn forgetting_removes_every_part_and_absent_is_not_an_error() {
        let slots = Small::with_limit(16);
        put(&slots, "k", &"x".repeat(40), LIMIT).unwrap();
        put(&slots, "other", "\"kept\"", LIMIT).unwrap();
        forget(&slots, "k").unwrap();
        assert_eq!(slots.names(), ["other"]);
        forget(&slots, "k").unwrap();
        assert_eq!(get(&slots, "k").unwrap(), None);
    }

    #[test]
    fn a_missing_part_or_a_damaged_head_is_an_error_and_not_a_shorter_credential() {
        let slots = Small::with_limit(16);
        put(&slots, "k", &"x".repeat(40), LIMIT).unwrap();
        slots.remove("k#3").unwrap();
        assert!(get(&slots, "k").is_err());

        for head in [
            "porter-parts:",
            "porter-parts:1",
            "porter-parts:65",
            "porter-parts:-2",
        ] {
            let slots = Small::with_limit(usize::MAX);
            slots.write("k", head).unwrap();
            assert!(get(&slots, "k").is_err(), "{head}");
            // And it can still be forgotten, which is how the user gets out of it.
            forget(&slots, "k").unwrap();
            assert!(slots.names().is_empty(), "{head}");
        }
    }
}

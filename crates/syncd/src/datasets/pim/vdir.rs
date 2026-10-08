//! The vdir on disk (vdirsyncer's storage spec): a collection is a directory, an item is one
//! file, `<uid>.ics` or `<uid>.vcf`, and the collection's `displayname` and `color` are two
//! small files beside them.
//!
//! Every file is written whole under a temporary name and renamed over its place, so a reader
//! that lists the directory sees a file complete or not at all: the temporary names start with a
//! dot and end in `.tmp`, matching neither `*.ics` nor `*.vcf`.

use super::PimKind;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// The file of a collection's name.
pub const DISPLAYNAME: &str = "displayname";
/// The file of a collection's colour.
pub const COLOR: &str = "color";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// What a collection says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meta {
    /// The name shown for it.
    pub displayname: Option<String>,
    /// Its colour as the server wrote it (`#0082c9FF`); contacts have none.
    pub color: Option<String>,
}

/// Writes `bytes` to `dir/name` atomically: a temporary file in the same directory, renamed
/// into place. Not flushed to disk: the mirror is the server's copy, and a file a crash left
/// short fails the hash check the mirror runs at open and is fetched again.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let temp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        std::fs::rename(&temp, dir.join(name))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Removes `dir/name`; a file already gone is not an error.
pub fn remove_file(dir: &Path, name: &str) -> std::io::Result<()> {
    match std::fs::remove_file(dir.join(name)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Makes `dir/file` say `value`, or not exist when there is none; untouched when it already
/// says so (a watcher is not woken for nothing).
fn keep(dir: &Path, file: &str, value: Option<&str>) -> std::io::Result<()> {
    let held = std::fs::read(dir.join(file)).ok();
    match (value, held) {
        (Some(want), Some(have)) if have == want.as_bytes() => Ok(()),
        (Some(want), _) => write_atomic(dir, file, want.as_bytes()),
        (None, Some(_)) => remove_file(dir, file),
        (None, None) => Ok(()),
    }
}

/// Writes the collection's `displayname` and `color` files into `dir`, which must be there
/// (`PimMirror::open` makes it). A collection whose directory is gone is never made again here:
/// the supervisor writes these on every look, outside the engine's cycle, and an account removed
/// meanwhile had its mirror deleted, which this must not bring back (`NotFound`).
pub fn write_meta(dir: &Path, meta: &Meta) -> std::io::Result<()> {
    keep(dir, DISPLAYNAME, meta.displayname.as_deref())?;
    keep(dir, COLOR, meta.color.as_deref())
}

fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._@+=-".contains(c)
}

/// A name is a file of the collection when it is one plain path component that does not start
/// with a dot.
pub fn is_item_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with('.') && name.chars().all(plain)
}

/// The item files of a collection directory, by name: those with the kind's extension.
pub fn items_in(dir: &Path, kind: PimKind) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n.ends_with(kind.extension()))
        .collect();
    names.sort();
    names
}

/// The `UID` an iCalendar or vCard text names: the first `UID` property, unfolded.
pub fn uid_of(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line
            .get(..4)
            .filter(|head| head.eq_ignore_ascii_case("UID:") || head.eq_ignore_ascii_case("UID;"))
            .map(|_| &line[3..])
        else {
            continue;
        };
        // `UID;X-PARAM=1:value` keeps what follows the first colon.
        let mut value = rest
            .split_once(':')
            .map_or(String::new(), |(_, v)| v.to_owned());
        while let Some(next) = lines.peek() {
            match next.strip_prefix([' ', '\t']) {
                Some(more) => {
                    value.push_str(more);
                    lines.next();
                }
                None => break,
            }
        }
        let value = value.trim().to_owned();
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// The file name an item gets: its UID when that is a safe name, else the server's own file
/// name, with the kind's extension.
pub fn file_name(kind: PimKind, uid: Option<&str>, server_name: &str) -> String {
    let ext = kind.extension();
    let stem = |text: &str| {
        let cleaned: String = text
            .chars()
            .map(|c| if plain(c) { c } else { '_' })
            .collect();
        cleaned.trim_start_matches('.').to_owned()
    };
    let server = server_name.rsplit('/').next().unwrap_or(server_name);
    let server = server.strip_suffix(ext).unwrap_or(server);
    let safe_uid = uid
        .filter(|u| !u.is_empty() && u.len() <= 120 && !u.starts_with('.') && u.chars().all(plain));
    let chosen = match safe_uid {
        Some(uid) => uid.to_owned(),
        None => match stem(server) {
            s if s.is_empty() => "item".to_owned(),
            s => s,
        },
    };
    format!("{chosen}{ext}")
}

/// Whether `bytes` is a whole iCalendar or vCard (what a reader must always find): it opens with
/// the kind's `BEGIN` line and closes with its `END` line.
pub fn is_complete(kind: PimKind, bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let (begin, end) = kind.envelope();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next();
    let last = lines.next_back();
    first.is_some_and(|l| l.eq_ignore_ascii_case(begin))
        && last.is_some_and(|l| l.eq_ignore_ascii_case(end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_uid_is_the_first_uid_property_unfolded() {
        const CASES: &[(&str, Option<&str>)] = &[
            ("BEGIN:VEVENT\r\nUID:abc-1\r\nEND:VEVENT\r\n", Some("abc-1")),
            (
                "BEGIN:VCARD\nUID:urn:uuid:1234\nEND:VCARD",
                Some("urn:uuid:1234"),
            ),
            (
                "BEGIN:VEVENT\r\nUID:long\r\n -tail\r\nSUMMARY:x\r\n",
                Some("long-tail"),
            ),
            ("uid:lower\r\n", Some("lower")),
            ("UID;X-FOO=1:param\r\n", Some("param")),
            ("SUMMARY:UIDnot\r\n", None),
            ("UID:\r\n", None),
            ("", None),
        ];
        for (text, want) in CASES {
            assert_eq!(uid_of(text).as_deref(), *want, "{text:?}");
        }
    }

    #[test]
    fn a_file_is_named_by_its_uid_or_else_by_the_servers_name() {
        const CASES: &[(PimKind, Option<&str>, &str, &str)] = &[
            (PimKind::Calendar, Some("abc-1"), "x.ics", "abc-1.ics"),
            (PimKind::Contacts, Some("abc-1"), "x.vcf", "abc-1.vcf"),
            (
                PimKind::Calendar,
                None,
                "server-name.ics",
                "server-name.ics",
            ),
            (PimKind::Calendar, Some("../evil"), "ok.ics", "ok.ics"),
            (PimKind::Calendar, Some(".hidden"), "ok.ics", "ok.ics"),
            (PimKind::Calendar, Some("a/b"), "ok.ics", "ok.ics"),
            (PimKind::Calendar, Some("sp ace"), "a b.ics", "a_b.ics"),
            (PimKind::Calendar, None, "..", "item.ics"),
            (PimKind::Calendar, None, "sub/dir.ics", "dir.ics"),
            (
                PimKind::Calendar,
                Some("a@b+c=d.e"),
                "z.ics",
                "a@b+c=d.e.ics",
            ),
        ];
        for (kind, uid, server, want) in CASES {
            assert_eq!(file_name(*kind, *uid, server), *want, "{uid:?} {server}");
            assert!(is_item_name(want), "{want}");
        }
    }

    #[test]
    fn a_complete_item_opens_and_closes_with_its_envelope() {
        const CASES: &[(PimKind, &str, bool)] = &[
            (
                PimKind::Calendar,
                "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
                true,
            ),
            (PimKind::Calendar, "begin:vcalendar\nEND:VCALENDAR", true),
            (PimKind::Calendar, "BEGIN:VCALENDAR\r\nUID:x\r\n", false),
            (PimKind::Calendar, "", false),
            (PimKind::Contacts, "BEGIN:VCARD\nEND:VCARD\n", true),
            (PimKind::Contacts, "BEGIN:VCALENDAR\nEND:VCALENDAR\n", false),
        ];
        for (kind, text, ok) in CASES {
            assert_eq!(is_complete(*kind, text.as_bytes()), *ok, "{text:?}");
        }
    }

    #[test]
    fn an_atomic_write_leaves_the_whole_file_and_no_temporary() {
        let dir = scratch("atomic");
        write_atomic(&dir, "a.ics", b"one").expect("write");
        write_atomic(&dir, "a.ics", b"two!").expect("rewrite");
        assert_eq!(std::fs::read(dir.join("a.ics")).expect("read"), b"two!");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .map(|e| e.expect("e").file_name().into_string().expect("utf8"))
            .collect();
        assert_eq!(names, ["a.ics"]);
        assert!(write_atomic(&dir.join("missing"), "a.ics", b"x").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn metadata_files_follow_the_collection_and_are_not_touched_when_unchanged() {
        let dir = scratch("meta");
        let meta = Meta {
            displayname: Some("Work".into()),
            color: Some("#0082c9FF".into()),
        };
        write_meta(&dir, &meta).expect("meta");
        assert_eq!(
            std::fs::read_to_string(dir.join(DISPLAYNAME)).expect("name"),
            "Work"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(COLOR)).expect("color"),
            "#0082c9FF"
        );
        let before = std::fs::metadata(dir.join(COLOR))
            .expect("m")
            .modified()
            .expect("t");
        write_meta(&dir, &meta).expect("again");
        let after = std::fs::metadata(dir.join(COLOR))
            .expect("m")
            .modified()
            .expect("t");
        assert_eq!(before, after);
        write_meta(
            &dir,
            &Meta {
                displayname: Some("Home".into()),
                color: None,
            },
        )
        .expect("changed");
        assert_eq!(
            std::fs::read_to_string(dir.join(DISPLAYNAME)).expect("name"),
            "Home"
        );
        assert!(!dir.join(COLOR).exists(), "no colour, no file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn metadata_never_makes_a_collection_whose_directory_is_gone() {
        // The account was removed and its mirror deleted between the supervisor's look at the
        // collections and its write of their names: nothing comes back (rel-13 follow-up, the
        // pim_bus removal test that found the account's directory again).
        let root = scratch("meta-gone");
        let dir = root.join("account").join("personal");
        let meta = Meta {
            displayname: Some("Work".into()),
            color: Some("#0082c9FF".into()),
        };
        let refused = write_meta(&dir, &meta).expect_err("no directory");
        assert_eq!(refused.kind(), std::io::ErrorKind::NotFound);
        assert!(!root.join("account").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn listing_returns_only_the_kinds_visible_files() {
        let dir = scratch("list");
        for name in ["b.ics", "a.ics", "c.vcf", ".d.ics.1.tmp", "displayname"] {
            std::fs::write(dir.join(name), "x").expect("file");
        }
        assert_eq!(items_in(&dir, PimKind::Calendar), ["a.ics", "b.ics"]);
        assert_eq!(items_in(&dir, PimKind::Contacts), ["c.vcf"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! What stays on this computer and is never sent to the drive: the one list of it.
//!
//! - **Lock files** an app keeps beside a file it has open, which mean nothing anywhere else:
//!   LibreOffice's `.~lock.<name>#`, Microsoft Office's `~$<name>` and vim's swap files
//!   (`.<name>.swp`, `.swo`, `.swx`). They are shown here like any file and never uploaded.
//! - **The desktop's trash** at the top of the drive (`.Trash-<uid>`, and a shared `.Trash`),
//!   where Files and `gio trash` put what is moved to the trash on a drive of its own. It and
//!   everything in it stay here; an item moved into it goes to the drive's recycle bin.
//!
//! The list is kept short on purpose: anything not on it is sent.

/// Whether a file named `name` is a lock file an app keeps beside one it has open.
pub fn is_lock_file(name: &str) -> bool {
    (name.starts_with(".~lock.") && name.ends_with('#'))
        || name.starts_with("~$")
        || (name.starts_with('.')
            && [".swp", ".swo", ".swx"]
                .iter()
                .any(|swap| name.len() > swap.len() + 1 && name.ends_with(swap)))
}

/// Whether a folder at the top of the drive named `name` is the desktop's trash.
pub fn is_trash(name: &str) -> bool {
    name == ".Trash"
        || name
            .strip_prefix(".Trash-")
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_files_and_the_trash_are_named_by_one_short_list() {
        for lock in [
            ".~lock.Plan.odt#",
            "~$Plan.docx",
            ".notes.txt.swp",
            ".a.swo",
        ] {
            assert!(is_lock_file(lock), "{lock}");
        }
        for file in ["Plan.odt", ".swp", "lock.txt", ".bashrc", "x.swp"] {
            assert!(!is_lock_file(file), "{file}");
        }
        assert!(is_trash(".Trash-1000"));
        assert!(is_trash(".Trash"));
        assert!(!is_trash(".Trash-"));
        assert!(!is_trash("Trash"));
    }
}

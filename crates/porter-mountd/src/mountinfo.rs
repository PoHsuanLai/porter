//! What is mounted where, as `/proc/self/mountinfo` says, and ending a FUSE connection.

use std::path::{Path, PathBuf};

/// Where the kernel lists each FUSE connection (the `fusectl` file system).
pub const FUSE_CONNECTIONS: &str = "/sys/fs/fuse/connections";

/// One line of the table: where it is mounted, its device and the file system type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The mount point.
    pub point: PathBuf,
    /// The device's major and minor numbers.
    pub device: (u32, u32),
    /// The file system type (`fuse.porter` for a drive's folder).
    pub fstype: String,
}

impl Entry {
    /// Its FUSE connection's number, which names it under [`FUSE_CONNECTIONS`]: the kernel's
    /// device number (a FUSE mount's major is 0, so it is the minor).
    pub fn connection(&self) -> u64 {
        let (major, minor) = self.device;
        (u64::from(major) << 20) | u64::from(minor)
    }
}

/// Ends FUSE connection `connection` at once (`<connections>/<n>/abort`): every call waiting on
/// it fails, and whoever serves it sees it gone, even while a file there is still open. Only the
/// person who mounted it may, and only where `fusectl` is mounted and writable.
///
/// # Errors
/// The connection is not there, or its abort file cannot be written.
pub fn abort(connections: &Path, connection: u64) -> std::io::Result<()> {
    std::fs::write(connections.join(connection.to_string()).join("abort"), b"1")
}

/// The entries of a mountinfo table, in its order (a later mount at a point is above an
/// earlier one).
pub fn parse(table: &str) -> Vec<Entry> {
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let (major, minor) = fields.nth(2)?.split_once(':')?;
            let device = (major.parse().ok()?, minor.parse().ok()?);
            let point = fields.nth(1)?;
            // The optional fields end at a lone "-"; the type follows it.
            let (_, after) = line.split_once(" - ")?;
            let fstype = after.split(' ').next()?;
            Some(Entry {
                point: PathBuf::from(unescape(point)),
                device,
                fstype: unescape(fstype),
            })
        })
        .collect()
}

/// This process's table now (empty when it cannot be read).
pub fn read() -> Vec<Entry> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|table| parse(&table))
        .unwrap_or_default()
}

/// The topmost mount at `path` in `entries`, if any.
pub fn at<'a>(entries: &'a [Entry], path: &Path) -> Option<&'a Entry> {
    entries.iter().rev().find(|entry| entry.point == path)
}

/// A mountinfo field with its octal escapes (`\040` for a space) read back.
pub fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Ok(code) = u8::from_str_radix(&field[i + 1..i + 4], 8)
        {
            out.push(code);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_gives_its_point_and_type_with_escapes_read_back() {
        let table = "\
36 35 98:0 / /mnt1 rw,noatime master:1 - ext3 /dev/root rw,errors=continue
90 29 0:52 / /home/ada/Cloud/Google\\040Drive ro,nosuid,nodev,noatime shared:5 - fuse.porter porter ro,user_id=1000
91 29 0:53 / /home/ada/Cloud/Google\\040Drive rw - fuse.sshfs host: rw
";
        let entries = parse(table);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].fstype, "ext3");
        let point = Path::new("/home/ada/Cloud/Google Drive");
        assert_eq!(entries[1].point, point);
        assert_eq!(entries[1].fstype, "fuse.porter");
        assert_eq!(entries[1].connection(), 52);
        assert_eq!(entries[0].connection(), 98 << 20);
        assert_eq!(
            at(&entries, point).map(|e| e.fstype.as_str()),
            Some("fuse.sshfs")
        );
        assert_eq!(at(&entries, Path::new("/home/ada/Cloud")), None);
        assert_eq!(
            unescape("/home/ada/Cloud/Google\\040Drive"),
            "/home/ada/Cloud/Google Drive"
        );
    }

    #[test]
    fn an_abort_writes_one_to_the_connections_abort_file() {
        let dir = test_kit::Scratch::new("mountd-abort");
        std::fs::create_dir_all(dir.join("52")).expect("connection");
        abort(dir.path(), 52).expect("aborted");
        assert_eq!(std::fs::read(dir.join("52/abort")).expect("abort"), b"1");
        assert!(abort(dir.path(), 53).is_err());
    }
}

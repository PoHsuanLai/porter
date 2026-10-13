//! The sealed memfd an API key travels on to a process that may read it and not change it.

use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::io::{self, Seek, Write};
use std::os::fd::OwnedFd;

/// `key` on an anonymous file, read position at the start, sealed against every change: no write,
/// no growing, no shrinking, and no seal added or removed. A reader gets the key and cannot alter
/// what another reader of the same file sees.
pub fn sealed_key(key: &str) -> io::Result<OwnedFd> {
    let fd = memfd_create(
        "porter-api-key",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let mut file = std::fs::File::from(fd);
    file.write_all(key.as_bytes())?;
    file.rewind()?;
    let fd = OwnedFd::from(file);
    fcntl_add_seals(
        &fd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{fcntl_get_seals, ftruncate};
    use std::io::Read;

    #[test]
    fn a_sealed_key_reads_back_and_cannot_change() {
        let fd = sealed_key("sk-or-v1-abc").expect("memfd");
        assert_eq!(
            fcntl_get_seals(&fd).expect("seals"),
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
        );
        let mut file = std::fs::File::from(fd);
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read");
        assert_eq!(text, "sk-or-v1-abc");
        // Every way of altering it, and of lifting a seal, is refused.
        assert!(file.write_all(b"x").is_err());
        assert!(ftruncate(&file, 0).is_err());
        assert!(ftruncate(&file, 4096).is_err());
        assert!(fcntl_add_seals(&file, SealFlags::empty()).is_err());
        file.rewind().expect("rewind");
        let mut again = String::new();
        file.read_to_string(&mut again).expect("read again");
        assert_eq!(again, text);
    }
}

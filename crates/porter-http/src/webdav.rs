//! WebDAV's methods, which the `http` crate has no constant for: `PROPFIND` and `MKCOL` (RFC
//! 4918) and `REPORT` (RFC 3253, which CalDAV, CardDAV and sync-collection use).

use http::Method;

/// `PROPFIND`.
pub fn propfind() -> Method {
    extension(b"PROPFIND")
}

/// `REPORT`.
pub fn report() -> Method {
    extension(b"REPORT")
}

/// `MKCOL`.
pub fn mkcol() -> Method {
    extension(b"MKCOL")
}

fn extension(token: &'static [u8]) -> Method {
    Method::from_bytes(token).unwrap_or_else(|_| unreachable!("a WebDAV method is a token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_method_is_its_token() {
        assert_eq!(propfind(), "PROPFIND");
        assert_eq!(report(), "REPORT");
        assert_eq!(mkcol(), "MKCOL");
    }
}

//! The app's end of an authenticated relay (design/31 §4.4 `OpenAuthenticated`, porter PLAN G2):
//! a byte stream to one endpoint of a granted account, already authenticated, so an IMAP, SMTP
//! or HTTP engine never holds the password. The descriptor is out of band: a D-Bus `h` or an
//! `SCM_RIGHTS` descriptor (a Unix stream socket), and for an app hosting porter in process an
//! in-memory duplex.

use porter_core::stream::DuplexEnd;
#[cfg(unix)]
use std::os::fd::OwnedFd;

/// The app's end of the relay.
#[derive(Debug)]
pub enum AuthenticatedStream {
    /// A Unix stream socket the daemon holds the other end of.
    #[cfg(unix)]
    Fd(OwnedFd),
    /// An in-memory duplex, for an app hosting porter in process (and for Windows).
    Memory(DuplexEnd),
}

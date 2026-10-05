//! The loopback listener (feature `io`).

use crate::loopback::{AuthCode, LoopbackFault};
use crate::pkce::OAuthState;

/// A listener on 127.0.0.1 at a port the OS chose.
#[derive(Debug)]
pub struct LoopbackServer {
    port: u16,
}

impl LoopbackServer {
    /// Binds 127.0.0.1 on a free port.
    pub async fn bind() -> std::io::Result<Self> {
        todo!("bind a tokio TcpListener on 127.0.0.1:0 and keep it")
    }

    /// The port, for the redirect URI.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accepts one request within the wait limit, answers the person with a page, and returns
    /// the code if the state is `expected`.
    pub async fn wait(self, expected: &OAuthState) -> Result<AuthCode, LoopbackFault> {
        let _ = expected;
        todo!("accept once, read at most MAX_REDIRECT_BYTES, `parse_redirect`, write the page")
    }
}

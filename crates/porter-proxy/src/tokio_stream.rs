//! `ByteStream` over a tokio stream (feature `io`): a Unix socket, a TCP stream, a TLS stream,
//! tokio's own duplex.

use porter_core::stream::ByteStream;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A tokio stream as a [`ByteStream`].
#[derive(Debug)]
pub struct TokioStream<T>(pub T);

impl<T: AsyncRead + AsyncWrite + Unpin + Send> ByteStream for TokioStream<T> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.0.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bytes_cross_a_tokio_duplex_through_the_trait() {
        let (a, b) = tokio::io::duplex(64);
        let (mut app, mut relay) = (TokioStream(a), TokioStream(b));
        app.write_all(b"A001 NOOP\r\n").await.expect("write");
        let mut buf = [0u8; 32];
        let n = relay.read(&mut buf).await.expect("read");
        assert_eq!(&buf[..n], b"A001 NOOP\r\n");
        app.shutdown().await.expect("shutdown");
        assert_eq!(relay.read(&mut buf).await.expect("read"), 0);
    }
}

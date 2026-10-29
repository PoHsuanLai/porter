//! `Changes`: the one stream every watch of the client gives.

use crate::error::Malformed;
use futures_core::Stream;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

/// What a watch gives: the changes from now on, one item each, until the connection ends. It is a
/// [`Stream`] for those who want combinators and has [`Changes::next`] for those who do not.
///
/// An item is an error, [`Malformed::Signal`] or another, when the daemon sent a signal that is
/// not well formed; the stream goes on.
pub struct Changes<T> {
    stream: Pin<Box<dyn Stream<Item = Result<T, Malformed>> + Send>>,
}

impl<T> Changes<T> {
    /// A watch over `stream`. A transport builds one from the signals it hears.
    pub fn new(stream: impl Stream<Item = Result<T, Malformed>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
        }
    }

    /// A watch whose every signal is well formed (one that carries nothing to get wrong).
    pub fn of(stream: impl Stream<Item = T> + Send + 'static) -> Self
    where
        T: 'static,
    {
        Self::new(futures_util::StreamExt::map(stream, Ok))
    }

    /// The next change, or `None` when the connection has ended.
    pub async fn next(&mut self) -> Option<Result<T, Malformed>> {
        std::future::poll_fn(|cx| self.stream.as_mut().poll_next(cx)).await
    }
}

impl<T> Stream for Changes<T> {
    type Item = Result<T, Malformed>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<T> fmt::Debug for Changes<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Changes").finish_non_exhaustive()
    }
}

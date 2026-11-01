//! porter's file writer: the one blocking place that writes files, kept out of the pure
//! vocabulary crate (`porter-core`) so that crate reaches no file system.
//!
//! [`atomic::AtomicWrite`] replaces a file whole or not at all; [`atomic::AtomicFile`] is one
//! small file kept with its backup. Std only, no runtime, unless the `book` feature is named:
//! [`book::commit`] saves a kept document on the blocking pool.

pub mod atomic;
#[cfg(feature = "book")]
pub mod book;

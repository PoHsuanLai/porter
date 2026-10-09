//! porter's file writer: the one blocking place that writes files, kept out of the pure
//! vocabulary crate (`porter-core`) so that crate reaches no file system.
//!
//! [`atomic::AtomicWrite`] replaces a file whole or not at all; [`atomic::AtomicFile`] is one
//! small file kept with its backup. Std only, no runtime.

pub mod atomic;

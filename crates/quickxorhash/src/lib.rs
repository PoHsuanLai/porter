//! Microsoft's QuickXorHash, the content hash OneDrive and Microsoft Graph report for each file
//! (`file.hashes.quickXorHash`, base64 of the 20 bytes). The hash is 160 bits: each byte is
//! xored in 11 bits further round than the one before, and the length is xored into the last
//! 64 bits. Microsoft documents it with a reference implementation in C#; this one is checked
//! against that one's outputs.
//!
//! [`QuickXorHash`] implements the RustCrypto [`digest`] traits, so it is used like any other
//! hasher of that family:
//!
//! ```
//! use quickxorhash::{Digest, QuickXorHash};
//!
//! let mut hash = QuickXorHash::new();
//! hash.update(b"J");
//! let out = hash.finalize();
//! assert_eq!(out.len(), 20);
//! assert_eq!(out[0], b'J');
//! // The length, 1, is xored in at byte 12.
//! assert_eq!(out[12], 1);
//! ```
//!
//! Nothing here can fail, so the crate has no error type.

#![warn(missing_docs)]

pub use digest::{self, Digest};

use digest::consts::U20;
use digest::{FixedOutput, FixedOutputReset, HashMarker, Output, OutputSizeUser, Reset, Update};

/// The width in bits.
const WIDTH: u32 = 160;

/// How many bits on each byte goes.
const SHIFT: u32 = 11;

/// A QuickXorHash being computed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuickXorHash {
    /// The 160 bits: two cells of 64 and the low 32 bits of a third.
    cells: [u64; 3],
    /// Where the next byte goes, in bits.
    shift: u32,
    /// How many bytes so far.
    length: u64,
}

impl QuickXorHash {
    /// The hash as its 20 bytes, without consuming it.
    fn bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[..8].copy_from_slice(&self.cells[0].to_le_bytes());
        out[8..16].copy_from_slice(&self.cells[1].to_le_bytes());
        out[16..].copy_from_slice(&self.cells[2].to_le_bytes()[..4]);
        for (at, byte) in self.length.to_le_bytes().iter().enumerate() {
            out[12 + at] ^= byte;
        }
        out
    }
}

impl HashMarker for QuickXorHash {}

impl OutputSizeUser for QuickXorHash {
    type OutputSize = U20;
}

impl Update for QuickXorHash {
    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let cell = (self.shift / 64) as usize;
            let offset = self.shift % 64;
            let bits = if cell == 2 { 32 } else { 64 };
            let value = u64::from(byte);
            self.cells[cell] ^= value << offset;
            if offset > bits - 8 {
                let next = if cell == 2 { 0 } else { cell + 1 };
                self.cells[next] ^= value >> (bits - offset);
            }
            self.shift = (self.shift + SHIFT) % WIDTH;
        }
        self.length = self.length.wrapping_add(bytes.len() as u64);
    }
}

impl FixedOutput for QuickXorHash {
    fn finalize_into(self, out: &mut Output<Self>) {
        out.copy_from_slice(&self.bytes());
    }
}

impl FixedOutputReset for QuickXorHash {
    fn finalize_into_reset(&mut self, out: &mut Output<Self>) {
        out.copy_from_slice(&self.bytes());
        Reset::reset(self);
    }
}

impl Reset for QuickXorHash {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(text: &str) -> Vec<u8> {
        hex::decode(text).expect("hex")
    }

    /// Inputs and outputs of the reference implementation, as rclone's tests of its port list
    /// them (there in base64).
    const VECTORS: &[(&str, &str)] = &[
        ("", "0000000000000000000000000000000000000000"),
        (
            "bcf8280e339f3ba7e6ef54712f",
            "bcc0070a1c30834fece014e6750715e2f0020000",
        ),
        (
            "bed04e18810d1bbc90fcdef13563cd220cbae8693f23652bfd985d14d50af7fd45099baefca792",
            "f85829dc70629ad0b3500ca28a78f7a646ab6139",
        ),
        (
            "8f289bc494b33ba6626743b5a9eded1bf6ef20062cb2fba487db2e213e70132d4904835580f8aa76c4d6\
             d1c3a93f46947cfee682a2df00393323f9bf1a00ae561afca052eb382a13c75fe9b01396c2196d09b854\
             c79d48726f5a1fcb58e19ddc714fba04aa584242c2cb407f03d65c",
            "8597cb2229522130ba3777b7b50fe24a013392da",
        ),
        (
            "3dfc4c714774bc85ba55b1c6fee8ff6345baa84a0a9b2043d2761e6c429acca68a1be51a0ea044726423\
             6df41e56754bba874ca0fa7b3fb4d1d61b57e67d959e41e1db6c43ca827c0bf6502a248da907d7be1f37\
             8ee75fe1156bf5ec490ae77c52f822a7a69565368fac91d58d084c0a9fdd1271afaa0d2837939590cdaa\
             a805ef034b5e2940e180d33bd6c0c15410865f134e4766e96c3d62338767b93d1ecb82fe9685c41d32f4\
             7ea31e51c122d9ab202269e535a9307a70f3cf4c79eda070caadc0b29085181d67717e32602aff39a09c\
             4b938a8bfd34587fb0350537",
            "417fd81291b4803b26844a4275686cc43cec7d51",
        ),
    ];

    #[test]
    fn the_hash_is_the_reference_implementations_whole_and_in_pieces() {
        for (input, output) in VECTORS {
            let input = bytes(input);
            assert_eq!(
                hex::encode(QuickXorHash::digest(&input)),
                *output,
                "{} bytes",
                input.len()
            );
            for size in [1, 7, 20, 64] {
                let mut pieces = QuickXorHash::new();
                for piece in input.chunks(size) {
                    Digest::update(&mut pieces, piece);
                }
                assert_eq!(
                    hex::encode(pieces.finalize()),
                    *output,
                    "{} bytes by {size}",
                    input.len()
                );
            }
        }
    }

    #[test]
    fn a_reset_hash_is_a_new_one() {
        let mut hash = QuickXorHash::new();
        Digest::update(&mut hash, b"something");
        let first = hash.finalize_reset();
        assert_eq!(hash, QuickXorHash::default());
        Digest::update(&mut hash, b"something");
        assert_eq!(hash.finalize(), first);
    }
}

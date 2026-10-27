//! Prints the QuickXorHash of a file in hex. Graph sends the same 20 bytes in base64.
//!
//! `cargo run -p quickxorhash --example hash_a_file -- <path>`

use quickxorhash::{Digest, QuickXorHash};
use std::io::Read;

fn main() -> std::io::Result<()> {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: hash_a_file <path>");
        std::process::exit(2);
    };
    let mut file = std::fs::File::open(path)?;
    let mut hash = QuickXorHash::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            n => hash.update(&buffer[..n]),
        }
    }
    let out = hash.finalize();
    println!("{}", hex::encode(out));
    Ok(())
}

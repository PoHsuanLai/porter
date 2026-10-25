//! Reads a sync-collection reply from a file and prints what changed and how it ends.
//!
//! `cargo run -p dav-proto --example read_a_sync_report -- <reply.xml>`

use dav_proto::{SyncChange, SyncEnd, parse_multistatus, parse_sync_collection};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: read_a_sync_report <reply.xml>");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(path)?;
    let reply = parse_sync_collection(&parse_multistatus(&text)?);
    for change in &reply.changes {
        match change {
            SyncChange::Changed { href, etag } => println!("changed {href} {etag}"),
            SyncChange::Removed { href } => println!("removed {href}"),
        }
    }
    match &reply.end {
        SyncEnd::Complete(token) => println!("complete; next time from {token}"),
        SyncEnd::Truncated(token) => println!("cut short; continue now from {token}"),
        SyncEnd::Missing => println!("no token; list again"),
        _ => println!("an ending this example does not know"),
    }
    Ok(())
}

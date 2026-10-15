//! Embeds the catalog stoker ships, at the rev the workspace pins, for `testkit::pinned_stoker_catalog`.
//!
//! A test runs where the build's sources may not be (an archive run in a jail binds only the
//! repo and the target directory), so the files go into the test binary itself. The rev is read
//! from the workspace's `Cargo.lock` and the files from cargo's checkout of it; asking `cargo
//! metadata` from here could wait on the package cache the running build holds. A build that
//! finds no checkout (vendored sources) embeds nothing, and the helper says so when a test asks.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let lock = manifest.join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let rev = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|text| stoker_rev(&text));
    let files = rev
        .as_deref()
        .and_then(checkout)
        .map(|root| catalog_files(&root.join("catalog")))
        .unwrap_or_default();
    let mut code = String::from("&[\n");
    for (name, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        code.push_str(&format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    code.push_str("]\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("out dir"));
    std::fs::write(out.join("stoker_catalog.rs"), code).expect("write the embedded catalog");
    println!(
        "cargo:rustc-env=PORTER_STOKER_CATALOG_REV={}",
        rev.unwrap_or_default()
    );
}

/// The commit of stoker's crates in the lock: `source = "git+…/stoker?rev=…#<commit>"`.
fn stoker_rev(lock: &str) -> Option<String> {
    lock.lines()
        .filter_map(|line| line.trim().strip_prefix("source = \""))
        .find(|source| source.contains("/stoker?") || source.contains("/stoker#"))
        .and_then(|source| source.trim_end_matches('"').rsplit_once('#'))
        .map(|(_, commit)| commit.to_owned())
}

/// Cargo's checkout of stoker at `rev`: `$CARGO_HOME/git/checkouts/stoker-<hash>/<short rev>`.
fn checkout(rev: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    let short = rev.get(..7)?;
    std::fs::read_dir(home.join("git").join("checkouts"))
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("stoker-"))
        .map(|entry| entry.path().join(short))
        .find(|dir| dir.is_dir())
}

/// The catalog's `.toml` files, by name.
fn catalog_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|path| Some((path.file_name()?.to_str()?.to_owned(), path)))
        .collect();
    files.sort();
    files
}

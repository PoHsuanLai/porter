//! Lists `providers/*.toml` for `shipped.rs`: one `(id, text)` entry per file, in id order, so
//! adding a provider file is adding the file and nothing else.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dir = manifest.join("../../providers");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    ids.sort();
    let mut list = String::from("&[\n");
    for id in &ids {
        let path = dir.join(format!("{id}.toml"));
        writeln!(
            list,
            "    ({id:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("write to a string");
    }
    list.push_str("]\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("out dir")).join("shipped_files.rs");
    std::fs::write(out, list).expect("write the list");
}

//! The include guard of a crate's one integration-test binary (`tests/it/main.rs`): a file or a
//! module directory dropped into `tests/it/` but never declared with `mod` would silently not
//! run. Every `tests/it` binary has a one-line test that calls
//! [`every_module_is_declared`].

use std::path::Path;

/// Panics, naming them, when `tests_dir` (relative to `manifest_dir`, as `"tests/it"`) holds a
/// `*.rs` file or a directory with a `mod.rs` that `main_rs` (the text of its `main.rs`, from
/// `include_str!("main.rs")`) does not declare with `mod name;`.
///
/// # Panics
/// When a module is undeclared, or the directory cannot be read.
pub fn every_module_is_declared(manifest_dir: &str, tests_dir: &str, main_rs: &str) {
    let dir = Path::new(manifest_dir).join(tests_dir);
    let missing = undeclared(&dir, main_rs);
    assert!(
        missing.is_empty(),
        "{} holds modules main.rs does not declare: {missing:?}",
        dir.display()
    );
}

/// The modules in `dir` that `main_rs` does not declare, sorted.
fn undeclared(dir: &Path, main_rs: &str) -> Vec<String> {
    let declared: Vec<&str> = main_rs
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            line.strip_prefix("mod ")?.strip_suffix(';')
        })
        .collect();
    let mut missing: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("a directory entry").path();
            let name = path.file_stem()?.to_str()?.to_owned();
            let is_module = if path.is_dir() {
                path.join("mod.rs").exists()
            } else {
                path.extension().is_some_and(|ext| ext == "rs") && name != "main"
            };
            (is_module && !declared.contains(&name.as_str())).then_some(name)
        })
        .collect();
    missing.sort();
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("porter-fake-guard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn a_module_file_or_directory_main_does_not_declare_is_named() {
        let dir = scratch("undeclared");
        for file in ["main.rs", "declared.rs", "forgotten.rs", "data.json"] {
            std::fs::write(dir.join(file), "").expect("file");
        }
        for module in ["dir_declared", "dir_forgotten"] {
            std::fs::create_dir_all(dir.join(module)).expect("dir");
            std::fs::write(dir.join(module).join("mod.rs"), "").expect("mod.rs");
        }
        // A data directory without a mod.rs is not a module.
        std::fs::create_dir_all(dir.join("fixtures")).expect("dir");
        let main_rs = "//! docs\n\nmod declared;\n#[cfg(feature = \"x\")]\npub mod dir_declared;\n";
        assert_eq!(
            undeclared(&dir, main_rs),
            ["dir_forgotten".to_owned(), "forgotten".to_owned()]
        );
        let all = "mod declared;\nmod dir_declared;\nmod dir_forgotten;\nmod forgotten;\n";
        assert!(undeclared(&dir, all).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

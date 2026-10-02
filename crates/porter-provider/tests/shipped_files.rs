//! Every provider file the repo ships parses, round-trips through TOML, and lays over the
//! others by id.

use porter_provider::{ProviderSet, ProviderSpec, parse_provider};
use std::path::PathBuf;

fn shipped() -> Vec<(PathBuf, ProviderSpec)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../providers");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("providers directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("readable");
            let spec = parse_provider(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (path, spec)
        })
        .collect()
}

#[test]
fn every_shipped_provider_file_parses_and_is_named_by_its_id() {
    let files = shipped();
    assert_eq!(files.len(), 4);
    for (path, spec) in files {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        assert_eq!(spec.id.as_str(), stem, "{}", path.display());
    }
}

#[test]
fn a_provider_spec_round_trips_through_toml() {
    for (path, spec) in shipped() {
        let text = toml::to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            parse_provider(&text).as_ref(),
            Ok(&spec),
            "{}",
            path.display()
        );
    }
}

#[test]
fn a_user_file_replaces_the_system_file_of_the_same_id() {
    let specs: Vec<ProviderSpec> = shipped().into_iter().map(|(_, spec)| spec).collect();
    let mut mine = specs[0].clone();
    mine.label = "My own".into();
    let set = ProviderSet::layered(specs.clone(), vec![mine.clone()]);
    assert_eq!(set.specs().len(), specs.len());
    assert_eq!(set.get(&mine.id).map(|s| s.label.as_str()), Some("My own"));
}

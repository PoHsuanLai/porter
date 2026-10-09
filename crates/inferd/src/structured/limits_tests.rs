//! `[ai.structured]` as `inferd.toml` holds it: the file's reading into limits. The limits
//! themselves are `porter_turns::structured::limits`.

use super::limits::*;
use crate::config::InferdConfig;
use model_provider::{CharCount, Count};

fn resolved(text: &str) -> Resolved {
    InferdConfig::from_toml(text).expect("reads").ai.resolve()
}

#[test]
fn no_table_is_the_default_limits() {
    let said = resolved("");
    assert_eq!(said.limits.schema.open_text, CharCount(4096));
    assert_eq!(said.limits.schema.open_list, Count(256));
    assert_eq!(said.limits.schema.depth, Count(16));
    assert_eq!(said.limits.repairs.0, 1);
    assert!(said.rejected.is_empty());
}

#[test]
fn values_in_range_are_taken_at_both_ends() {
    let table = [
        ("open_text = 256", 256),
        ("open_text = 65536", 65536),
        ("open_text = 255", 4096),
        ("open_text = 65537", 4096),
        ("open_text = -1", 4096),
    ];
    for (line, want) in table {
        let said = resolved(&format!("[ai.structured]\n{line}\n"));
        assert_eq!(said.limits.schema.open_text, CharCount(want), "{line}");
        assert_eq!(
            said.rejected.is_empty(),
            want != 4096 || line == "open_text = 4096"
        );
    }
    let edges = resolved("[ai.structured]\nopen_list=16\ndepth=64\nrepair_budget=0\n");
    assert_eq!(edges.limits.schema.open_list, Count(16));
    assert_eq!(edges.limits.schema.depth, Count(64));
    assert_eq!(edges.limits.repairs.0, 0);
    let top = resolved("[ai.structured]\nopen_list=4096\ndepth=4\nrepair_budget=3\n");
    assert_eq!(top.limits.schema.open_list, Count(4096));
    assert_eq!(top.limits.schema.depth, Count(4));
    assert_eq!(top.limits.repairs.0, 3);
}

#[test]
fn a_bad_field_falls_back_alone_and_is_named() {
    let said = resolved("[ai.structured]\ndepth = 3\nrepair_budget = 9\nopen_list = 100\n");
    assert_eq!(said.limits.schema.depth, Count(16));
    assert_eq!(said.limits.repairs.0, 1);
    assert_eq!(said.limits.schema.open_list, Count(100));
    assert_eq!(
        said.rejected,
        vec!["ai.structured.depth", "ai.structured.repair_budget"]
    );
}

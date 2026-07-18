use std::{fs, path::PathBuf};

use commonkit_core::is_forbidden_path;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    path: String,
    forbidden: bool,
}

#[test]
fn preserves_the_node_forbidden_path_contract() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/node-commonkit/forbidden-paths/cases.json");
    let cases: Vec<Case> =
        serde_json::from_str(&fs::read_to_string(fixture).expect("fixture")).expect("cases");

    for case in cases {
        assert_eq!(
            is_forbidden_path(&case.path),
            case.forbidden,
            "{}",
            case.path
        );
    }
}

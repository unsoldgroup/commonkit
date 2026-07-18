use std::{fs, path::PathBuf};

#[test]
fn checked_in_layer_schema_matches_the_rust_contract() {
    assert_schema("layer.schema.json", commonkit_contracts::layer_schema());
    assert_schema("error.schema.json", commonkit_contracts::error_schema());
}

fn assert_schema(
    name: &str,
    generated: Result<serde_json::Value, commonkit_contracts::ContractError>,
) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("schemas")
        .join(name);
    let checked_in: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(path).unwrap_or_else(|_| panic!("checked-in schema {name}")),
    )
    .expect("valid schema JSON");
    assert_eq!(checked_in, generated.expect("generated schema"));
}

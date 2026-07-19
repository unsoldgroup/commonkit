use std::{fs, path::PathBuf};

#[test]
fn checked_in_layer_schema_matches_the_rust_contract() {
    assert_schema(
        "commonkit.schema.json",
        commonkit_contracts::commonkit_schema(),
    );
    assert_schema("layer.schema.json", commonkit_contracts::layer_schema());
    assert_schema("error.schema.json", commonkit_contracts::error_schema());
    assert_schema(
        "provenance.schema.json",
        commonkit_contracts::provenance_schema(),
    );
    assert_schema(
        "commonkit-lock.schema.json",
        commonkit_contracts::lock_schema(),
    );
    assert_schema("plan.schema.json", commonkit_contracts::plan_schema());
    assert_schema("receipt.schema.json", commonkit_contracts::receipt_schema());
    assert_schema(
        "diagnostics.schema.json",
        commonkit_contracts::diagnostics_schema(),
    );
    assert_schema(
        "execution-manifest.schema.json",
        commonkit_contracts::execution_manifest_schema(),
    );
    assert_schema(
        "execution-receipt.schema.json",
        commonkit_contracts::execution_receipt_schema(),
    );
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

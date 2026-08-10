use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("schemas"));
    fs::create_dir_all(&root)?;
    for (name, schema) in [
        (
            "commonkit.schema.json",
            commonkit_contracts::commonkit_schema()?,
        ),
        ("layer.schema.json", commonkit_contracts::layer_schema()?),
        ("error.schema.json", commonkit_contracts::error_schema()?),
        (
            "provenance.schema.json",
            commonkit_contracts::provenance_schema()?,
        ),
        (
            "commonkit-lock.schema.json",
            commonkit_contracts::lock_schema()?,
        ),
        ("plan.schema.json", commonkit_contracts::plan_schema()?),
        (
            "plan-v2.schema.json",
            commonkit_contracts::plan_v2_schema()?,
        ),
        (
            "receipt.schema.json",
            commonkit_contracts::receipt_schema()?,
        ),
        (
            "receipt-v2.schema.json",
            commonkit_contracts::receipt_v2_schema()?,
        ),
        (
            "diagnostics.schema.json",
            commonkit_contracts::diagnostics_schema()?,
        ),
        ("skills.schema.json", commonkit_contracts::skills_schema()?),
        (
            "execution-manifest.schema.json",
            commonkit_contracts::execution_manifest_schema()?,
        ),
        (
            "execution-receipt.schema.json",
            commonkit_contracts::execution_receipt_schema()?,
        ),
    ] {
        let mut output = serde_json::to_string_pretty(&schema)?;
        output.push('\n');
        fs::write(root.join(name), output)?;
    }
    let portable_root = root.join("portable-context");
    fs::create_dir_all(&portable_root)?;
    for entry in commonkit_contracts::portable_context::portable_context_schema_registry() {
        let mut output = serde_json::to_string_pretty(&entry.schema_value()?)?;
        output.push('\n');
        fs::write(
            portable_root.join(format!("{}.schema.json", entry.name)),
            output,
        )?;
    }
    Ok(())
}

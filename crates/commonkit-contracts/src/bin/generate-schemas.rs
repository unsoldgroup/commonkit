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
            "receipt.schema.json",
            commonkit_contracts::receipt_schema()?,
        ),
        (
            "diagnostics.schema.json",
            commonkit_contracts::diagnostics_schema()?,
        ),
        ("skills.schema.json", commonkit_contracts::skills_schema()?),
    ] {
        let mut output = serde_json::to_string_pretty(&schema)?;
        output.push('\n');
        fs::write(root.join(name), output)?;
    }
    Ok(())
}

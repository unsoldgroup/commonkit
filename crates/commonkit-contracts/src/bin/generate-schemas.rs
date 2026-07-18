use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("schemas"));
    fs::create_dir_all(&root)?;
    for (name, schema) in [
        ("layer.schema.json", commonkit_contracts::layer_schema()?),
        ("error.schema.json", commonkit_contracts::error_schema()?),
    ] {
        let mut output = serde_json::to_string_pretty(&schema)?;
        output.push('\n');
        fs::write(root.join(name), output)?;
    }
    Ok(())
}

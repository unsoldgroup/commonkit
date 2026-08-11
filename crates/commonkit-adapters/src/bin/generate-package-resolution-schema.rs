use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("schemas/package-resolution.schema.json"));
    let mut output =
        serde_json::to_string_pretty(&commonkit_adapters::package_resolution_schema()?)?;
    output.push('\n');
    fs::write(path, output)?;
    Ok(())
}

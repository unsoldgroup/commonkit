use std::error::Error;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use commonkit_config::{LayerSet, MergeRules, compose_layers};
use commonkit_contracts::{LayerDocument, assert_no_embedded_secrets};
use commonkit_platform::AppPaths;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "commonkit",
    version,
    about = "Portable developer environment control plane"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report local runtime paths and contract versions.
    Status,
    /// Compose ordered layer documents and emit normalized state.
    Compose {
        #[arg(long = "layer", required = true)]
        layers: Vec<PathBuf>,
    },
    /// Explain which layers contributed to a JSON pointer.
    Explain {
        pointer: String,
        #[arg(long = "layer", required = true)]
        layers: Vec<PathBuf>,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("commonkit: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Command::Status => {
            let paths = AppPaths::discover()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "contractVersion": commonkit_contracts::CONTRACT_VERSION,
                    "schemaVersion": commonkit_contracts::SCHEMA_VERSION,
                    "runtimeVersion": env!("CARGO_PKG_VERSION"),
                    "configDirectory": paths.config,
                    "stateDirectory": paths.state,
                    "cacheDirectory": paths.cache,
                }))?
            );
        }
        Command::Compose { layers } => {
            let result = compose(&layers)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "spec": result.spec,
                    "specDigest": result.spec_digest,
                    "trace": result.trace,
                    "lock": result.lock,
                }))?
            );
        }
        Command::Explain { pointer, layers } => {
            let result = compose(&layers)?;
            let entry = result
                .trace
                .entries
                .get(&pointer)
                .ok_or_else(|| format!("no provenance for JSON pointer {pointer}"))?;
            println!("{}", serde_json::to_string_pretty(entry)?);
        }
    }
    Ok(())
}

fn compose(paths: &[PathBuf]) -> Result<commonkit_config::CompositionResult, Box<dyn Error>> {
    let mut layers = Vec::with_capacity(paths.len());
    for path in paths {
        let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        assert_no_embedded_secrets(&value)?;
        layers.push(serde_json::from_value::<LayerDocument>(value)?);
    }
    let layers = LayerSet::new(layers)?;
    Ok(compose_layers(&layers, &MergeRules::new())?)
}

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
    /// Initialize CommonKit's local private runtime directories.
    Init,
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
    /// Plan synchronization through the local CommonKit daemon.
    Sync {
        #[arg(long)]
        confirmed: bool,
    },
    /// Show the deterministic changes in a synchronization plan.
    Diff,
    /// Apply a content-addressed plan through the local daemon.
    Apply {
        plan_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    /// Verify managed state and provider integrity.
    Verify,
    /// Roll back a durable run receipt.
    Rollback {
        run_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    /// Configure scheduled drift checks.
    Schedule,
    /// Export redacted diagnostics from the local daemon.
    Diagnostics,
    /// Inspect or configure the persistent MCP relay.
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
}

#[derive(Subcommand)]
enum RelayCommand {
    /// Read relay configuration and managed-upstream status.
    Status,
    /// Plan reconciliation from a versioned service request document.
    Reconcile {
        request: PathBuf,
        #[arg(long)]
        confirmed: bool,
    },
    /// Request a controlled relay runtime restart.
    Restart {
        #[arg(long)]
        confirmed: bool,
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
        Command::Init => {
            let paths = AppPaths::discover()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "status": "initialized",
                    "configDirectory": paths.config,
                    "stateDirectory": paths.state,
                    "cacheDirectory": paths.cache,
                }))?
            );
        }
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
        Command::Apply {
            confirmed: false, ..
        }
        | Command::Rollback {
            confirmed: false, ..
        }
        | Command::Sync { confirmed: false } => {
            return Err(
                "confirmation_required: pass --confirmed after reviewing the operation".into(),
            );
        }
        Command::Sync { confirmed: true } => print_daemon(daemon_control(
            "POST",
            "/control/v1/sync/plan",
            Some(
                json!({"confirmed":true,"confirmationId":"cli-sync","idempotencyKey":nonce("sync")}),
            ),
            None,
        )?)?,
        Command::Apply {
            plan_id,
            confirmed: true,
        } => print_daemon(daemon_control(
            "POST",
            &format!("/control/v1/plans/{plan_id}/apply"),
            Some(json!({"confirmed":true,"confirmationId":"cli-apply"})),
            Some(nonce("apply")),
        )?)?,
        Command::Rollback {
            run_id,
            confirmed: true,
        } => print_daemon(daemon_control(
            "POST",
            "/control/v1/rollback",
            Some(
                json!({"runId":run_id,"confirmed":true,"confirmationId":"cli-rollback","idempotencyKey":nonce("rollback")}),
            ),
            None,
        )?)?,
        Command::Verify => print_daemon(daemon_control(
            "POST",
            "/control/v1/verify",
            Some(json!({"targetId":null,"pointer":null})),
            None,
        )?)?,
        Command::Schedule => {
            print_daemon(daemon_control("GET", "/control/v1/schedule", None, None)?)?
        }
        Command::Diagnostics => print_daemon(daemon_control(
            "GET",
            "/control/v1/diagnostics",
            None,
            None,
        )?)?,
        Command::Diff => {
            return Err("plan_required: diff requires an explicit durable plan identifier".into());
        }
        Command::Relay { command } => run_relay(command)?,
    }
    Ok(())
}

fn nonce(prefix: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    format!("{prefix}-{}-{now}", std::process::id())
}

fn daemon_control(
    method: &str,
    path: &str,
    body: Option<Value>,
    idempotency_key: Option<String>,
) -> Result<Value, Box<dyn Error>> {
    let paths = AppPaths::discover()?;
    let discovery_bytes = std::fs::read(paths.state.join("daemon.json"))
        .map_err(|_| "daemon_unavailable: CommonKit daemon discovery is unavailable")?;
    let discovery: commonkit_service::DaemonDiscovery = serde_json::from_slice(&discovery_bytes)?;
    let token = commonkit_service::ControlToken::load(&paths.config.join("control.token"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let url = format!("http://127.0.0.1:{}{path}", discovery.port);
    let mut request = if method == "GET" {
        client.get(url)
    } else {
        client.post(url)
    }
    .bearer_auth(token.expose_for_client());
    if let Some(key) = idempotency_key {
        request = request.header("idempotency-key", key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send()?;
    let status = response.status();
    let value: Value = response.json()?;
    if !status.is_success() {
        let code = value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("daemon_error");
        return Err(format!("{code}: CommonKit daemon rejected the request").into());
    }
    Ok(value)
}

fn print_daemon(value: Value) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn run_relay(command: RelayCommand) -> Result<(), Box<dyn Error>> {
    let (method, path, body) = match command {
        RelayCommand::Status => ("GET", "/control/v1/relay", None),
        RelayCommand::Reconcile {
            confirmed: false, ..
        }
        | RelayCommand::Restart { confirmed: false } => {
            return Err(
                "confirmation_required: pass --confirmed after reviewing the operation".into(),
            );
        }
        RelayCommand::Reconcile {
            request,
            confirmed: true,
        } => {
            let mut value: Value = serde_json::from_slice(&std::fs::read(request)?)?;
            value["confirmed"] = Value::Bool(true);
            ("POST", "/control/v1/relay/reconcile", Some(value))
        }
        RelayCommand::Restart { confirmed: true } => (
            "POST",
            "/control/v1/relay/restart",
            Some(json!({"confirmed": true})),
        ),
    };
    let paths = AppPaths::discover()?;
    let discovery: commonkit_service::DaemonDiscovery =
        serde_json::from_slice(&std::fs::read(paths.state.join("daemon.json"))?)?;
    let token = commonkit_service::ControlToken::load(&paths.config.join("control.token"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let url = format!("http://127.0.0.1:{}{}", discovery.port, path);
    let request = if method == "GET" {
        client.get(url)
    } else {
        client.post(url)
    }
    .bearer_auth(token.expose_for_client());
    let response = if let Some(body) = body {
        request.json(&body).send()?
    } else {
        request.send()?
    };
    let status = response.status();
    let value: Value = response.json()?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    if !status.is_success() {
        return Err(format!("daemon_request_failed: HTTP {}", status.as_u16()).into());
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

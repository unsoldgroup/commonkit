use std::error::Error;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use commonkit_config::{LayerSet, MergeRules, compose_layers};
use commonkit_contracts::{LayerDocument, assert_no_embedded_secrets};
use commonkit_platform::AppPaths;
use commonkit_skills::{PromotionPlan, PromotionReceipt, ProviderUpgradeReport, ScheduleKind};
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
    Init {
        #[command(subcommand)]
        command: Option<InitCommand>,
    },
    /// Report local runtime paths and contract versions.
    Status,
    /// Compose ordered layer documents and emit normalized state.
    Compose {
        #[arg(long = "layer")]
        layers: Vec<PathBuf>,
    },
    /// Explain which layers contributed to a JSON pointer.
    Explain {
        pointer: String,
        #[arg(long = "layer")]
        layers: Vec<PathBuf>,
    },
    /// Plan synchronization through the local CommonKit daemon.
    Sync {
        #[arg(long)]
        confirmed: bool,
    },
    /// Show the deterministic changes in a synchronization plan.
    Diff { plan_id: String },
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
    /// Inspect or provision credential references through the local daemon.
    Credentials {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    /// Inspect or configure the persistent MCP relay.
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
    /// Inspect canonical skills and durable optimization candidates.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Subcommand)]
enum InitCommand {
    Create(InitArgs),
    Connect(InitArgs),
}

#[derive(clap::Args)]
struct InitArgs {
    #[arg(long)]
    repository: String,
    #[arg(long)]
    kit_directory: PathBuf,
    #[arg(long)]
    loadout: String,
    #[arg(long)]
    target: String,
    #[arg(long)]
    target_root: PathBuf,
}

#[derive(Subcommand)]
enum CredentialCommand {
    Readiness {
        references: Vec<String>,
    },
    Apply {
        destination_ids: Vec<String>,
        #[arg(long)]
        confirmed: bool,
    },
    Verify {
        destination_ids: Vec<String>,
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

#[derive(Subcommand)]
enum SkillsCommand {
    List {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
    },
    Candidate {
        #[command(subcommand)]
        command: CandidateCommand,
    },
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommand,
    },
    Opportunities {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long, default_value_t = 3)]
        minimum_evidence: u64,
    },
    Optimize {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        suite: PathBuf,
        #[arg(long)]
        environment: PathBuf,
        #[arg(long)]
        tasks: PathBuf,
        #[arg(long)]
        harness: PathBuf,
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value = "mock")]
        backend: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        confirmed: bool,
    },
    Promote {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        candidate: String,
        #[arg(long)]
        approver: String,
        #[arg(long)]
        reason: String,
        #[arg(long = "repository-revision")]
        repository_revision: String,
        #[arg(long)]
        confirmed: bool,
    },
    ApplyPromotion {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        confirmed: bool,
    },
    Rollback {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        receipt: PathBuf,
        #[arg(long)]
        confirmed: bool,
    },
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Schedule {
        #[command(subcommand)]
        command: SkillScheduleCommand,
    },
}

#[derive(Subcommand)]
enum CandidateCommand {
    List {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
    },
    Show {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        id: String,
    },
}

#[derive(Subcommand)]
enum EvidenceCommand {
    Preview {
        file: PathBuf,
    },
    Import {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        skill: String,
        #[arg(long = "source-kind", default_value = "manual_failure")]
        source_kind: String,
        #[arg(long, default_value = "local_only")]
        consent: String,
        #[arg(long = "created-at-unix-ms")]
        created_at_unix_ms: u64,
        #[arg(long = "delete-after-unix-ms")]
        delete_after_unix_ms: u64,
        #[arg(long)]
        confirmed: bool,
        file: PathBuf,
    },
}

#[derive(Subcommand)]
enum ProviderCommand {
    Check {
        #[arg(long)]
        environment: PathBuf,
        #[arg(long = "lock")]
        lock_file: PathBuf,
    },
    Upgrade {
        #[arg(long)]
        uv: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long = "lock")]
        lock_file: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
        #[arg(long)]
        harness: PathBuf,
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        confirmed: bool,
    },
    Activate {
        #[arg(long)]
        uv: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        approver: String,
        #[arg(long)]
        reason: String,
        #[arg(long = "active-lock")]
        active_lock: PathBuf,
        #[arg(long)]
        confirmed: bool,
    },
}

#[derive(Subcommand)]
enum SkillScheduleCommand {
    Status {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
    },
    Enable {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
        #[arg(long, default_value = "discovery")]
        kind: String,
        #[arg(long)]
        interval_seconds: u64,
        #[arg(long, default_value_t = 0)]
        maximum_cost_micros: u64,
        #[arg(long)]
        confirmed: bool,
    },
    Disable {
        #[arg(long)]
        repository: PathBuf,
        #[arg(long)]
        state: PathBuf,
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
        Command::Init { command: None } => {
            let paths = AppPaths::discover()?;
            paths.create_private_roots()?;
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
        Command::Init {
            command: Some(command),
        } => {
            use commonkit_cli::onboarding::{InitMode, InitRequest, ProcessRunner, initialize};
            let paths = AppPaths::discover()?;
            let (mode, args) = match command {
                InitCommand::Create(args) => (InitMode::Create, args),
                InitCommand::Connect(args) => (InitMode::Connect, args),
            };
            let result = initialize(
                &InitRequest {
                    mode,
                    repository: args.repository,
                    kit_directory: args.kit_directory,
                    loadout: args.loadout,
                    target: args.target,
                    target_root: args.target_root,
                    config_directory: paths.config,
                    state_directory: paths.state,
                },
                &ProcessRunner::from_path(),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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
            if layers.is_empty() {
                print_daemon(daemon_control("GET", "/control/v1/compose", None, None)?)?;
                return Ok(());
            }
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
            if layers.is_empty() {
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/explain",
                    Some(json!({"pointer":pointer})),
                    None,
                )?)?;
                return Ok(());
            }
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
        Command::Diff { plan_id } => print_daemon(daemon_control(
            "GET",
            &format!("/control/v1/plans/{plan_id}"),
            None,
            None,
        )?)?,
        Command::Credentials { command } => run_credentials(command)?,
        Command::Relay { command } => run_relay(command)?,
        Command::Skills { command } => run_skills(command)?,
    }
    Ok(())
}

fn require_skill_confirmation(confirmed: bool) -> Result<(), Box<dyn Error>> {
    if confirmed {
        Ok(())
    } else {
        Err("confirmation_required: pass --confirmed after reviewing the operation".into())
    }
}

fn run_skills(command: SkillsCommand) -> Result<(), Box<dyn Error>> {
    match command {
        SkillsCommand::List { repository, state } => {
            let _ = (repository, state);
            print_daemon(daemon_control("GET", "/control/v1/skills", None, None)?)?
        }
        SkillsCommand::Candidate { command } => match command {
            CandidateCommand::List { repository, state } => {
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "GET",
                    "/control/v1/skills/candidates",
                    None,
                    None,
                )?)?
            }
            CandidateCommand::Show {
                repository,
                state,
                id,
            } => {
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "GET",
                    &format!("/control/v1/skills/candidates/{id}"),
                    None,
                    None,
                )?)?
            }
        },
        SkillsCommand::Evidence { command } => match command {
            EvidenceCommand::Preview { file } => {
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/skills/evidence/preview",
                    Some(json!({"content":String::from_utf8(std::fs::read(file)?)?})),
                    None,
                )?)?;
            }
            EvidenceCommand::Import {
                repository,
                state,
                skill,
                source_kind,
                consent,
                created_at_unix_ms,
                delete_after_unix_ms,
                confirmed,
                file,
            } => {
                require_skill_confirmation(confirmed)?;
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/skills/evidence/import",
                    Some(json!({
                        "confirmed": true, "confirmationId": nonce("skill-evidence"), "skillId": skill,
                        "sourceKind": source_kind, "consent": consent,
                        "retention": {"deleteAfterUnixMs": delete_after_unix_ms},
                        "createdAtUnixMs": created_at_unix_ms,
                        "content": String::from_utf8(std::fs::read(file)?)?,
                    })),
                    None,
                )?)?;
            }
        },
        SkillsCommand::Opportunities {
            repository,
            state,
            minimum_evidence,
        } => {
            let _ = (repository, state);
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/opportunities",
                Some(json!({"minimumEvidence":minimum_evidence})),
                None,
            )?)?
        }
        SkillsCommand::Optimize {
            repository,
            state,
            manifest,
            suite,
            environment,
            tasks,
            harness,
            corpus,
            backend,
            model,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            let manifest: commonkit_contracts::SkillOptimizationManifest =
                read_json_file(&manifest)?;
            let suite: commonkit_contracts::SkillEvaluationSuite = read_json_file(&suite)?;
            let _ = (repository, state);
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/optimize",
                Some(json!({
                    "confirmed": true, "confirmationId": nonce("skill-optimize"), "manifest": manifest,
                    "suite": suite, "environment": environment, "tasks": tasks, "harness": harness,
                    "corpus": corpus, "backend": backend, "model": model,
                })),
                None,
            )?)?;
        }
        SkillsCommand::Promote {
            repository,
            state,
            candidate,
            approver,
            reason,
            repository_revision,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            let _ = (repository, state);
            print_daemon(daemon_control(
                "POST",
                &format!("/control/v1/skills/candidates/{candidate}/promotion-plans"),
                Some(json!({
                    "confirmed": true, "confirmationId": nonce("skill-promote"), "repositoryRevision": repository_revision,
                    "approval": {"approver": approver, "approvedAtUnixMs": now_unix_ms()?, "reason": reason}
                })),
                None,
            )?)?;
        }
        SkillsCommand::ApplyPromotion {
            repository,
            state,
            plan,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            let _ = (repository, state);
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/promotions/apply",
                Some(
                    json!({"confirmed":true,"confirmationId":nonce("skill-apply"),"plan":read_json_file::<PromotionPlan>(&plan)?}),
                ),
                None,
            )?)?;
        }
        SkillsCommand::Rollback {
            repository,
            state,
            receipt,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            let _ = (repository, state);
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/promotions/rollback",
                Some(
                    json!({"confirmed":true,"confirmationId":nonce("skill-rollback"),"receipt":read_json_file::<PromotionReceipt>(&receipt)?}),
                ),
                None,
            )?)?;
        }
        SkillsCommand::Provider { command } => match command {
            ProviderCommand::Check {
                environment,
                lock_file,
            } => print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/provider/check",
                Some(
                    json!({"environment":environment,"providerLock":read_json_file::<commonkit_contracts::ProviderLock>(&lock_file)?}),
                ),
                None,
            )?)?,
            ProviderCommand::Upgrade {
                uv,
                root,
                lock_file,
                fixtures,
                harness,
                corpus,
                report,
                confirmed,
            } => {
                require_skill_confirmation(confirmed)?;
                let result = daemon_control(
                    "POST",
                    "/control/v1/skills/provider/upgrade",
                    Some(
                        json!({"confirmed":true,"confirmationId":nonce("skill-provider-upgrade"),"uv":uv,"root":root,"providerLock":read_json_file::<commonkit_contracts::ProviderLock>(&lock_file)?,"fixtures":fixtures,"harness":harness,"corpus":corpus}),
                    ),
                    None,
                )?;
                std::fs::write(report, serde_json::to_vec_pretty(&result)?)?;
                print_daemon(result)?;
            }
            ProviderCommand::Activate {
                uv,
                root,
                report,
                approver,
                reason,
                active_lock,
                confirmed,
            } => {
                require_skill_confirmation(confirmed)?;
                let report: ProviderUpgradeReport = read_json_file(&report)?;
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/skills/provider/activate",
                    Some(
                        json!({"confirmed":true,"confirmationId":nonce("skill-provider-activate"),"uv":uv,"root":root,"report":report,"approver":approver,"approvedAtUnixMs":now_unix_ms()?,"reason":reason,"activeLock":active_lock}),
                    ),
                    None,
                )?)?;
            }
        },
        SkillsCommand::Schedule { command } => match command {
            SkillScheduleCommand::Status { repository, state } => {
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "GET",
                    "/control/v1/skills/schedule",
                    None,
                    None,
                )?)?
            }
            SkillScheduleCommand::Enable {
                repository,
                state,
                kind,
                interval_seconds,
                maximum_cost_micros,
                confirmed,
            } => {
                require_skill_confirmation(confirmed)?;
                let kind = match kind.as_str() {
                    "discovery" => ScheduleKind::Discovery,
                    "candidate_generation" => ScheduleKind::CandidateGeneration,
                    _ => return Err(format!("unsupported schedule kind: {kind}").into()),
                };
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/skills/schedule",
                    Some(
                        json!({"confirmed":true,"confirmationId":nonce("skill-schedule"),"enabled":true,"kind":kind,"intervalSeconds":interval_seconds,"maximumCostMicrosPerPeriod":maximum_cost_micros}),
                    ),
                    None,
                )?)?;
            }
            SkillScheduleCommand::Disable {
                repository,
                state,
                confirmed,
            } => {
                require_skill_confirmation(confirmed)?;
                let _ = (repository, state);
                print_daemon(daemon_control(
                    "POST",
                    "/control/v1/skills/schedule",
                    Some(
                        json!({"confirmed":true,"confirmationId":nonce("skill-schedule"),"enabled":false,"kind":"discovery","intervalSeconds":1,"maximumCostMicrosPerPeriod":0}),
                    ),
                    None,
                )?)?;
            }
        },
    }
    Ok(())
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn now_unix_ms() -> Result<u64, Box<dyn Error>> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
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

fn run_credentials(command: CredentialCommand) -> Result<(), Box<dyn Error>> {
    let value = match command {
        CredentialCommand::Readiness { references } => daemon_control(
            "POST",
            "/control/v1/credentials/readiness",
            Some(json!({"references": references})),
            None,
        )?,
        CredentialCommand::Apply {
            confirmed: false, ..
        } => {
            return Err(
                "confirmation_required: pass --confirmed after reviewing the operation".into(),
            );
        }
        CredentialCommand::Apply {
            destination_ids,
            confirmed: true,
        } => daemon_control(
            "POST",
            "/control/v1/credentials/apply",
            Some(json!({
                "destinationIds": destination_ids,
                "confirmed": true,
                "confirmationId": "cli-credentials-apply"
            })),
            None,
        )?,
        CredentialCommand::Verify { destination_ids } => daemon_control(
            "POST",
            "/control/v1/credentials/verify",
            Some(json!({"destinationIds": destination_ids})),
            None,
        )?,
    };
    print_daemon(value)
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

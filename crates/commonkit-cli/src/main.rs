use std::error::Error;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use commonkit_config::{LayerSet, MergeRules, compose_layers};
use commonkit_contracts::{LayerDocument, assert_no_embedded_secrets};
use commonkit_platform::AppPaths;
use commonkit_skills::{
    Approval, EvidenceImport, PromotionPlan, PromotionReceipt, ProviderUpgradeReport, ScheduleKind,
    SkillEngine, SkillOptBackend, SkillOptProviderConfig, SkillOptProviderManager,
    SkillOptSleepOptimizer, SkillSchedule, UpgradeApproval, check_skillopt_provider,
};
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
    /// Inspect canonical skills and durable optimization candidates.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
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
            print_json(&SkillEngine::open(repository, state)?.inventory()?)?
        }
        SkillsCommand::Candidate { command } => match command {
            CandidateCommand::List { repository, state } => {
                print_json(&SkillEngine::open(repository, state)?.candidates()?)?
            }
            CandidateCommand::Show {
                repository,
                state,
                id,
            } => print_json(
                &SkillEngine::open(repository, state)?
                    .candidate(&commonkit_contracts::StableId::parse(id)?)?,
            )?,
        },
        SkillsCommand::Evidence { command } => match command {
            EvidenceCommand::Preview { file } => {
                let scratch = std::env::temp_dir()
                    .join(format!("commonkit-evidence-preview-{}", std::process::id()));
                let engine = SkillEngine::open(std::env::current_dir()?, &scratch)?;
                print_json(&engine.preview_evidence(&std::fs::read(file)?)?)?;
                let _ = std::fs::remove_dir_all(scratch);
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
                let engine = SkillEngine::open(repository, state)?;
                print_json(&engine.import_evidence(EvidenceImport {
                    skill_id: commonkit_contracts::StableId::parse(skill)?,
                    source_kind: parse_evidence_source(&source_kind)?,
                    consent: parse_evidence_consent(&consent)?,
                    retention: commonkit_contracts::EvidenceRetention {
                        delete_after_unix_ms,
                    },
                    created_at_unix_ms,
                    bytes: std::fs::read(file)?,
                })?)?;
            }
        },
        SkillsCommand::Opportunities {
            repository,
            state,
            minimum_evidence,
        } => print_json(&SkillEngine::open(repository, state)?.opportunities(minimum_evidence)?)?,
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
            let optimizer = SkillOptSleepOptimizer::new(SkillOptProviderConfig {
                environment_root: environment,
                provider_lock: manifest.provider.clone(),
                backend: parse_backend(&backend)?,
                model,
                tasks_file: tasks,
                harness_executable: harness,
                harness_corpus: corpus,
                environment: provider_environment(),
                timeout_seconds: manifest.limits.timeout_seconds,
            })?;
            print_json(
                &SkillEngine::open(repository, state)?.optimize(&manifest, &suite, &optimizer)?,
            )?;
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
            print_json(&SkillEngine::open(repository, state)?.plan_promotion(
                &commonkit_contracts::StableId::parse(candidate)?,
                Approval {
                    approver: commonkit_contracts::StableId::parse(approver)?,
                    approved_at_unix_ms: now_unix_ms()?,
                    reason,
                },
                commonkit_contracts::GitRevision::parse(repository_revision)?,
            )?)?;
        }
        SkillsCommand::ApplyPromotion {
            repository,
            state,
            plan,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            print_json(
                &SkillEngine::open(repository, state)?.apply_promotion(&read_json_file::<
                    PromotionPlan,
                >(
                    &plan
                )?)?,
            )?;
        }
        SkillsCommand::Rollback {
            repository,
            state,
            receipt,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            print_json(
                &SkillEngine::open(repository, state)?.rollback_promotion(&read_json_file::<
                    PromotionReceipt,
                >(
                    &receipt
                )?)?,
            )?;
        }
        SkillsCommand::Provider { command } => match command {
            ProviderCommand::Check {
                environment,
                lock_file,
            } => print_json(&check_skillopt_provider(
                &environment,
                &read_json_file(&lock_file)?,
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
                let manager =
                    SkillOptProviderManager::open(uv, root)?.with_harness(harness, corpus)?;
                let result = manager.execute_upgrade(
                    &manager.plan_upgrade(read_json_file(&lock_file)?, fixtures)?,
                )?;
                std::fs::write(report, serde_json::to_vec_pretty(&result)?)?;
                print_json(&result)?;
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
                let manager = SkillOptProviderManager::open(uv, root)?;
                let report: ProviderUpgradeReport = read_json_file(&report)?;
                manager.activate_upgrade(
                    &report,
                    UpgradeApproval {
                        approver: commonkit_contracts::StableId::parse(approver)?,
                        approved_at_unix_ms: now_unix_ms()?,
                        reason,
                    },
                    active_lock,
                )?;
                print_json(&json!({"activated": true, "provider": report.provider_lock}))?;
            }
        },
        SkillsCommand::Schedule { command } => match command {
            SkillScheduleCommand::Status { repository, state } => {
                print_json(&SkillEngine::open(repository, state)?.schedule_status()?)?
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
                print_json(&SkillEngine::open(repository, state)?.configure_schedule(
                    SkillSchedule {
                        kind,
                        enabled: true,
                        interval_seconds,
                        maximum_cost_micros_per_period: maximum_cost_micros,
                    },
                )?)?;
            }
            SkillScheduleCommand::Disable {
                repository,
                state,
                confirmed,
            } => {
                require_skill_confirmation(confirmed)?;
                print_json(&SkillEngine::open(repository, state)?.disable_schedule()?)?;
            }
        },
    }
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn parse_backend(value: &str) -> Result<SkillOptBackend, Box<dyn Error>> {
    Ok(match value {
        "mock" => SkillOptBackend::Mock,
        "claude" => SkillOptBackend::Claude,
        "codex" => SkillOptBackend::Codex,
        "handoff" => SkillOptBackend::Handoff,
        "azure_openai" => SkillOptBackend::AzureOpenAi,
        _ => return Err(format!("unsupported SkillOpt backend: {value}").into()),
    })
}
fn parse_evidence_source(
    value: &str,
) -> Result<commonkit_contracts::EvidenceSourceKind, Box<dyn Error>> {
    Ok(match value {
        "manual_failure" => commonkit_contracts::EvidenceSourceKind::ManualFailure,
        "claude_session" => commonkit_contracts::EvidenceSourceKind::ClaudeSession,
        "codex_session" => commonkit_contracts::EvidenceSourceKind::CodexSession,
        "evaluation_case" => commonkit_contracts::EvidenceSourceKind::EvaluationCase,
        _ => return Err(format!("unsupported evidence source: {value}").into()),
    })
}
fn parse_evidence_consent(
    value: &str,
) -> Result<commonkit_contracts::EvidenceConsent, Box<dyn Error>> {
    Ok(match value {
        "local_only" => commonkit_contracts::EvidenceConsent::LocalOnly,
        "approved_for_provider" => commonkit_contracts::EvidenceConsent::ApprovedForProvider,
        "approved_portable" => commonkit_contracts::EvidenceConsent::ApprovedPortable,
        _ => return Err(format!("unsupported evidence consent: {value}").into()),
    })
}
fn provider_environment() -> std::collections::BTreeMap<String, String> {
    [
        "ANTHROPIC_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_AUTH_MODE",
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_API_VERSION",
        "OPENAI_API_KEY",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
    .collect()
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

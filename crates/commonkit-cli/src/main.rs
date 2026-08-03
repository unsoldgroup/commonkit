use std::collections::BTreeMap;
use std::error::Error;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use commonkit_cli::about_me_setup::{SetupAnswers, SetupRequest, setup_profile};
use commonkit_config::{LayerSet, compose_layers, v1_merge_rules};
use commonkit_contracts::{
    LayerDocument, LayerKind, SchemaVersion, SecurityPolicy, Sha256Digest, StableId,
    assert_no_embedded_secrets,
};
use commonkit_core::enforce_policy_floor;
use commonkit_personal_context::{
    EncryptedRevisionStore, FieldOperation, ProfileFieldId, RevisionBinding, SecretValue,
    encrypt_revision_for_recipient_strings,
};
use commonkit_platform::AppPaths;
use commonkit_skills::{PromotionPlan, PromotionReceipt, ProviderUpgradeReport, ScheduleKind};
use serde_json::{Value, json};
use zeroize::Zeroizing;

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
    /// Internal authenticated stdio bridge used by generated MCP clients.
    #[command(hide = true)]
    RelayClient,
    /// Initialize CommonKit's local private runtime directories.
    Init {
        #[command(subcommand)]
        command: Option<InitCommand>,
    },
    /// Report local runtime paths and contract versions.
    Status,
    /// Install and manage the unattended per-user CommonKit daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// List and select configured local and SSH targets.
    Targets {
        #[command(subcommand)]
        command: TargetCommand,
    },
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
        /// Fetch and validate the trusted Git remote before creating the plan.
        #[arg(long)]
        fetch: bool,
        // Accepted for compatibility with older automation. Preview is read-only
        // and no longer asks the operator for mutation confirmation.
        #[arg(long, hide = true)]
        confirmed: bool,
    },
    /// Show the deterministic changes in a synchronization plan.
    Diff { plan_id: String },
    /// Report what a plan's loadout costs an agent on every turn.
    Budget { plan_id: String },
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
    Schedule {
        #[command(subcommand)]
        command: ScheduleCommand,
    },
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
    /// Create, inspect, restore, and promote mutable-state snapshots.
    Snapshots {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
    /// Inspect canonical skills and durable optimization candidates.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    /// Inspect the work-profile schema or encrypt a confirmed headless revision.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Search and retrieve authorized context through the local daemon.
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    /// Read and contribute to the encrypted owner profile.
    AboutMe {
        #[command(subcommand)]
        command: AboutMeCommand,
    },
}

#[derive(Subcommand)]
enum ContextCommand {
    Search {
        #[arg(long)]
        session_id: String,
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    Retrieve {
        #[arg(long)]
        session_id: String,
        section_id: String,
        #[arg(long, default_value_t = 65_536)]
        max_bytes: u32,
    },
    RequestAccess {
        #[arg(long)]
        session_id: String,
        descriptor_id: String,
        #[arg(long)]
        purpose: String,
        #[arg(long, default_value_t = 3_600)]
        duration_seconds: u32,
    },
    InspectReceipt {
        #[arg(long)]
        session_id: String,
        receipt_id: String,
        #[arg(long, value_parser = ["user", "organization"])]
        audience: String,
    },
    ProposeProfileRevision {
        #[arg(long)]
        session_id: String,
        #[arg(long = "field-id", required = true)]
        field_ids: Vec<String>,
        #[arg(long)]
        rationale: String,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Print the closed CommonKit work-profile schema.
    Schema,
    /// Encrypt a confirmed JSON object and stage ciphertext for synchronization.
    Encrypt {
        #[arg(long)]
        answers: PathBuf,
        #[arg(long = "recipient")]
        recipients: Vec<String>,
        #[arg(long)]
        profile_id: String,
        #[arg(long)]
        revision_id: String,
        #[arg(long = "parent-hash")]
        parent_hashes: Vec<String>,
        #[arg(long)]
        confirmed: bool,
    },
}

#[derive(Subcommand)]
enum AboutMeCommand {
    /// Create an encrypted profile through a short, reviewable interview.
    Setup {
        #[arg(long, default_value = "personal")]
        loadout: String,
        #[arg(long, default_value = "default")]
        project: String,
        #[arg(long, default_value = "commonkit-agent")]
        agent: String,
    },
    Status,
    Summary,
    Draft {
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        summary: String,
        /// JSON object with topicKey, category, and text. Repeat for each claim.
        #[arg(long = "claim-json")]
        claims: Vec<String>,
    },
    Publish {
        draft_id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        confirmed: bool,
    },
    Suggestions,
    Decide {
        suggestion_id: String,
        #[arg(long)]
        decision: String,
        #[arg(long)]
        confirmed: bool,
    },
    Search {
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: u8,
    },
    Suggest {
        #[arg(long)]
        topic_key: String,
        #[arg(long)]
        category: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        evidence_quote: String,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    Install,
    Start,
    Status,
    Restart,
    ReloadDomains {
        #[arg(long)]
        confirmed: bool,
    },
    Uninstall,
}

#[derive(Subcommand)]
enum TargetCommand {
    List,
    Select {
        targets: Vec<String>,
        #[arg(long)]
        confirmed: bool,
    },
    Plan {
        targets: Vec<String>,
        #[arg(long)]
        confirmed: bool,
    },
    Apply {
        target: String,
        plan_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    Verify {
        targets: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ScheduleCommand {
    Status,
    Enable {
        #[arg(long)]
        interval_seconds: u64,
        #[arg(long)]
        confirmed: bool,
    },
    Disable {
        #[arg(long)]
        confirmed: bool,
    },
}

#[derive(Subcommand)]
enum SnapshotCommand {
    Create {
        database_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    List,
    Restore {
        snapshot_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    Promote {
        database_id: String,
        target_id: String,
        #[arg(long)]
        confirmed: bool,
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
    /// Optional project-loadout layer ID, applied after the personal kit.
    #[arg(long)]
    project_loadout: Option<String>,
    /// Optional target-override layer ID, applied last.
    #[arg(long)]
    target_override: Option<String>,
    #[arg(long)]
    target: String,
    #[arg(long)]
    target_root: PathBuf,
    /// Desired-state provider used by this loadout.
    #[arg(long, value_enum, default_value = "native")]
    provider: InitProvider,
    /// Exact provider version (required for APM and chezmoi).
    #[arg(long)]
    provider_version: Option<String>,
    #[arg(long)]
    provider_executable: Option<PathBuf>,
    #[arg(long)]
    apm_manifest: Option<PathBuf>,
    #[arg(long)]
    apm_lockfile: Option<PathBuf>,
    #[arg(long)]
    apm_policy: Option<PathBuf>,
    #[arg(long)]
    chezmoi_source: Option<PathBuf>,
    #[arg(long)]
    chezmoi_config: Option<PathBuf>,
    /// Commit and push the portable target registration (required for connect).
    #[arg(long)]
    publish_registration: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum InitProvider {
    Native,
    Apm,
    Chezmoi,
}

#[derive(Subcommand)]
enum CredentialCommand {
    Readiness {
        references: Vec<String>,
    },
    /// Build a redacted, content-addressed credential provisioning plan.
    Plan {
        destination_ids: Vec<String>,
    },
    /// Apply a previously reviewed credential plan.
    Apply {
        #[arg(long)]
        plan_id: String,
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
        #[arg(long)]
        backend: String,
        /// Permit the deterministic mock optimizer for local development only.
        #[arg(long)]
        allow_mock_backend: bool,
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
    CanaryApply {
        #[arg(long)]
        deployment: PathBuf,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        confirmed: bool,
    },
    CanaryRollback {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        deployment_receipt_id: String,
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
        Command::RelayClient => run_relay_client()?,
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
            use commonkit_cli::onboarding::{
                InitMode, InitRequest, ProcessRunner, ProviderSelection, initialize,
            };
            let paths = AppPaths::discover()?;
            let (mode, args) = match command {
                InitCommand::Create(args) => (InitMode::Create, args),
                InitCommand::Connect(args) => (InitMode::Connect, args),
            };
            let provider = match args.provider {
                InitProvider::Native => {
                    if let Some(version) = &args.provider_version {
                        if version != env!("CARGO_PKG_VERSION") {
                            return Err(format!(
                                "native provider version must be exactly {}",
                                env!("CARGO_PKG_VERSION")
                            )
                            .into());
                        }
                    }
                    ProviderSelection::Native
                }
                InitProvider::Apm => {
                    require_pin(args.provider_version.as_deref(), "0.25.0", "APM")?;
                    ProviderSelection::Apm {
                        executable: required_path(
                            args.provider_executable,
                            "--provider-executable",
                        )?,
                        manifest: required_path(args.apm_manifest, "--apm-manifest")?,
                        lockfile: required_path(args.apm_lockfile, "--apm-lockfile")?,
                        policy: required_path(args.apm_policy, "--apm-policy")?,
                    }
                }
                InitProvider::Chezmoi => {
                    require_pin(args.provider_version.as_deref(), "2.70.4", "chezmoi")?;
                    ProviderSelection::Chezmoi {
                        executable: required_path(
                            args.provider_executable,
                            "--provider-executable",
                        )?,
                        source: required_path(args.chezmoi_source, "--chezmoi-source")?,
                        config: required_path(args.chezmoi_config, "--chezmoi-config")?,
                    }
                }
            };
            let result = initialize(
                &InitRequest {
                    mode,
                    repository: args.repository,
                    kit_directory: args.kit_directory,
                    loadout: args.loadout,
                    project_loadout: args.project_loadout,
                    target_override: args.target_override,
                    target: args.target,
                    target_root: args.target_root,
                    config_directory: paths.config,
                    state_directory: paths.state,
                    provider,
                    publish_registration: args.publish_registration,
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
        Command::Daemon { command } => run_daemon_lifecycle(command)?,
        Command::Targets { command } => {
            match command {
                TargetCommand::List => {
                    print_daemon(daemon_control("GET", "/control/v1/targets", None, None)?)?
                }
                TargetCommand::Select {
                    confirmed: false, ..
                } => {
                    return Err("confirmation_required: pass --confirmed after reviewing the target selection".into());
                }
                TargetCommand::Select {
                    targets,
                    confirmed: true,
                } => print_daemon(daemon_control(
                    "POST",
                    "/control/v1/targets/select",
                    Some(
                        json!({"targets":targets,"confirmed":true,"confirmationId":"cli-target-select"}),
                    ),
                    None,
                )?)?,
                TargetCommand::Plan {
                    confirmed: false, ..
                } => {
                    return Err(
                        "confirmation_required: each target plan requires --confirmed".into(),
                    );
                }
                TargetCommand::Plan {
                    targets,
                    confirmed: true,
                } => {
                    if targets.is_empty() {
                        return Err("at least one target is required".into());
                    }
                    let mut plans = Vec::with_capacity(targets.len());
                    for target in targets {
                        plans.push(daemon_control(
                            "POST",
                            &format!("/control/v1/targets/{target}/sync/plan"),
                            Some(json!({"confirmed":true,"confirmationId":format!("cli-plan-{target}")})),
                            None,
                        )?);
                    }
                    print_daemon(Value::Array(plans))?;
                }
                TargetCommand::Verify { targets } => {
                    if targets.is_empty() {
                        return Err("at least one target is required".into());
                    }
                    let mut results = Vec::with_capacity(targets.len());
                    for target in targets {
                        results.push(daemon_control(
                            "POST",
                            &format!("/control/v1/targets/{target}/verify"),
                            Some(json!({})),
                            None,
                        )?);
                    }
                    print_daemon(Value::Array(results))?;
                }
                TargetCommand::Apply {
                    confirmed: false, ..
                } => {
                    return Err("confirmation_required: target apply requires --confirmed".into());
                }
                TargetCommand::Apply {
                    target,
                    plan_id,
                    confirmed: true,
                } => print_daemon(daemon_control(
                    "POST",
                    &format!("/control/v1/targets/{target}/plans/{plan_id}/apply"),
                    Some(json!({"confirmed":true,"confirmationId":format!("cli-apply-{target}")})),
                    Some(nonce("target-apply")),
                )?)?,
            }
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
        } => {
            return Err(
                "confirmation_required: pass --confirmed after reviewing the operation".into(),
            );
        }
        Command::Sync { fetch, confirmed } => print_daemon(daemon_control(
            "POST",
            "/control/v1/sync/plan",
            Some(
                json!({"confirmed":confirmed || fetch,"confirmationId":"cli-sync","idempotencyKey":nonce("sync"),"fetch":fetch}),
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
        Command::Schedule { command } => run_schedule(command)?,
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
        Command::Budget { plan_id } => print_daemon(daemon_control(
            "GET",
            &format!("/control/v1/plans/{plan_id}/budget"),
            None,
            None,
        )?)?,
        Command::Credentials { command } => run_credentials(command)?,
        Command::Relay { command } => run_relay(command)?,
        Command::Snapshots { command } => run_snapshots(command)?,
        Command::Skills { command } => run_skills(command)?,
        Command::Profile { command } => run_profile(command)?,
        Command::Context { command } => run_context(command)?,
        Command::AboutMe { command } => run_about_me(command)?,
    }
    Ok(())
}

fn run_context(command: ContextCommand) -> Result<(), Box<dyn Error>> {
    let (path, body) = match command {
        ContextCommand::Search {
            session_id,
            query,
            limit,
        } => {
            if limit == 0 || limit > 100 || query.len() > 1_024 {
                return Err("context search requires limit 1..=100 and query <= 1024 bytes".into());
            }
            (
                "/control/v1/context/search",
                json!({"sessionId":session_id,"query":query,"limit":limit}),
            )
        }
        ContextCommand::Retrieve {
            session_id,
            section_id,
            max_bytes,
        } => {
            if max_bytes == 0 || max_bytes > 4 * 1024 * 1024 {
                return Err("context retrieval requires max-bytes 1..=4194304".into());
            }
            (
                "/control/v1/context/sections/retrieve",
                json!({"sessionId":session_id,"sectionId":section_id,"maxBytes":max_bytes}),
            )
        }
        ContextCommand::RequestAccess {
            session_id,
            descriptor_id,
            purpose,
            duration_seconds,
        } => {
            if duration_seconds == 0 || duration_seconds > 30 * 24 * 60 * 60 {
                return Err("context access duration must be 1..=2592000 seconds".into());
            }
            (
                "/control/v1/context/access-requests",
                json!({"sessionId":session_id,"descriptorId":descriptor_id,"purpose":purpose,"durationSeconds":duration_seconds}),
            )
        }
        ContextCommand::InspectReceipt {
            session_id,
            receipt_id,
            audience,
        } => (
            "/control/v1/context/receipts/inspect",
            json!({"sessionId":session_id,"receiptId":receipt_id,"audience":audience}),
        ),
        ContextCommand::ProposeProfileRevision {
            session_id,
            field_ids,
            rationale,
        } => {
            if field_ids.len() > 64 || rationale.len() > 4_096 {
                return Err("profile proposal exceeds the local request bound".into());
            }
            (
                "/control/v1/context/profile-revision-proposals",
                json!({"sessionId":session_id,"fieldIds":field_ids,"rationale":rationale}),
            )
        }
    };
    print_daemon(daemon_control("POST", path, Some(body), None)?)?;
    Ok(())
}

fn run_profile(command: ProfileCommand) -> Result<(), Box<dyn Error>> {
    match command {
        ProfileCommand::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &commonkit_contracts::portable_context::core_profile_schema()
                )?
            );
        }
        ProfileCommand::Encrypt {
            confirmed: false, ..
        } => {
            return Err(
                "confirmation_required: review every profile value and pass --confirmed".into(),
            );
        }
        ProfileCommand::Encrypt {
            answers,
            recipients,
            profile_id,
            revision_id,
            parent_hashes,
            confirmed: true,
        } => {
            if recipients.len() < 3 {
                return Err("three distinct public recipients are required: device, offline recovery, and password manager".into());
            }
            let recipients = recipients
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            if recipients.len() < 3 {
                return Err("three distinct public recipients are required".into());
            }
            let bytes = Zeroizing::new(std::fs::read(&answers)?);
            if bytes.len() > 1024 * 1024 {
                return Err("profile answers exceed the local input bound".into());
            }
            let answers: BTreeMap<String, String> = serde_json::from_slice(&bytes)?;
            let schema = commonkit_contracts::portable_context::core_profile_schema();
            if answers
                .keys()
                .any(|field| !schema.fields.contains_key(field))
            {
                return Err(
                    "profile answers contain a field outside the closed core schema".into(),
                );
            }
            let fields = answers
                .into_iter()
                .map(|(field, value)| {
                    Ok((
                        ProfileFieldId::parse(field)?,
                        FieldOperation::Set(SecretValue::from_string(value)),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, commonkit_personal_context::CryptoError>>()?;
            let binding = RevisionBinding {
                schema_version: SchemaVersion(1),
                profile_id: StableId::parse(profile_id)?,
                profile_schema_id: schema.id,
                profile_schema_version: schema.version,
                revision_id: StableId::parse(revision_id)?,
                parent_hashes: parent_hashes
                    .into_iter()
                    .map(Sha256Digest::parse)
                    .collect::<Result<Vec<_>, _>>()?,
            };
            let encrypted = encrypt_revision_for_recipient_strings(
                binding,
                fields,
                &recipients.into_iter().collect::<Vec<_>>(),
            )?;
            let paths = AppPaths::discover()?;
            paths.create_private_roots()?;
            let store = EncryptedRevisionStore::open(paths.state.join("personal-context/staged"))?;
            let digest = store.stage(&encrypted)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "revisionId": encrypted.binding.revision_id,
                    "ciphertextDigest": digest,
                    "staged": true,
                }))?
            );
        }
    }
    Ok(())
}

fn run_about_me(command: AboutMeCommand) -> Result<(), Box<dyn Error>> {
    let value = match command {
        AboutMeCommand::Setup {
            loadout,
            project,
            agent,
        } => return run_about_me_setup(loadout, project, agent),
        AboutMeCommand::Status => daemon_control("GET", "/control/v1/about-me", None, None)?,
        AboutMeCommand::Summary => {
            daemon_control("GET", "/control/v1/about-me/summary", None, None)?
        }
        AboutMeCommand::Draft {
            expected_revision,
            summary,
            claims,
        } => {
            let claims = claims
                .iter()
                .map(|claim| serde_json::from_str::<Value>(claim))
                .collect::<Result<Vec<_>, _>>()?;
            daemon_control(
                "POST",
                "/control/v1/about-me/drafts",
                Some(json!({
                    "expectedRevision":expected_revision,
                    "summary":summary,
                    "claims":claims
                })),
                None,
            )?
        }
        AboutMeCommand::Publish {
            draft_id,
            expected_revision,
            confirmed,
        } => {
            if !confirmed {
                return Err("publishing an About Me draft requires --confirmed".into());
            }
            daemon_control(
                "POST",
                "/control/v1/about-me/drafts/publish",
                Some(json!({
                    "draftId":draft_id,
                    "expectedRevision":expected_revision,
                    "confirmed":true,
                    "confirmationId":nonce("about-me-publish")
                })),
                None,
            )?
        }
        AboutMeCommand::Suggestions => daemon_control(
            "GET",
            "/control/v1/about-me/suggestions/pending",
            None,
            None,
        )?,
        AboutMeCommand::Decide {
            suggestion_id,
            decision,
            confirmed,
        } => {
            if !confirmed || !matches!(decision.as_str(), "accept" | "reject") {
                return Err("decision must be accept or reject and requires --confirmed".into());
            }
            daemon_control(
                "POST",
                "/control/v1/about-me/suggestions/decide",
                Some(json!({
                    "suggestionId":suggestion_id,
                    "decision":decision,
                    "confirmed":true,
                    "confirmationId":nonce("about-me-suggestion-decision")
                })),
                None,
            )?
        }
        AboutMeCommand::Search { query, limit } => daemon_control(
            "POST",
            "/control/v1/about-me/search",
            Some(json!({"query":query,"categories":[],"limit":limit})),
            None,
        )?,
        AboutMeCommand::Suggest {
            topic_key,
            category,
            text,
            evidence_quote,
        } => daemon_control(
            "POST",
            "/control/v1/about-me/suggestions",
            Some(json!({
                "topicKey":topic_key,
                "category":category,
                "text":text,
                "evidenceQuote":evidence_quote
            })),
            None,
        )?,
    };
    print_daemon(value)
}

fn run_about_me_setup(
    loadout: String,
    project: String,
    agent: String,
) -> Result<(), Box<dyn Error>> {
    println!("Let’s create your private About Me profile.");
    println!("Press Enter to skip anything. Nothing is saved until you approve.\n");
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout();
    let answers = SetupAnswers {
        name: ask(&mut input, &mut output, "What should I call you?")?,
        explanation_style: ask(
            &mut input,
            &mut output,
            "How should I explain unfamiliar or technical things?",
        )?,
        decision_style: ask(
            &mut input,
            &mut output,
            "When there are choices, how should I help you decide?",
        )?,
        tools: ask(
            &mut input,
            &mut output,
            "Which tools, languages, or areas do you use often?",
        )?,
        constraints: ask(
            &mut input,
            &mut output,
            "What recurring limits or working rules should I remember?",
        )?,
        never_assume: ask(
            &mut input,
            &mut output,
            "What should an agent never assume about you?",
        )?,
    };

    println!("\nReview what CommonKit will remember:");
    for (label, value) in [
        ("Name", &answers.name),
        ("Explanations", &answers.explanation_style),
        ("Decisions", &answers.decision_style),
        ("Tools and experience", &answers.tools),
        ("Working rules", &answers.constraints),
        ("Never assume", &answers.never_assume),
    ] {
        if !value.trim().is_empty() {
            println!("  {label}: {}", value.trim());
        }
    }
    let approved = ask(
        &mut input,
        &mut output,
        "\nSave this encrypted profile? [yes/no]",
    )?;
    if !matches!(approved.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("Canceled. Nothing was saved.");
        return Ok(());
    }

    let paths = AppPaths::discover()?;
    let outcome = setup_profile(SetupRequest {
        config_directory: paths.config,
        state_directory: paths.state,
        loadout_id: loadout,
        project_id: project,
        agent_id: agent,
        answers,
        approved: true,
    })?;
    println!(
        "Profile saved as revision {}. Restart or reload the CommonKit daemon, then run `commonkit about-me summary`.",
        outcome.revision
    );
    Ok(())
}

fn ask(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
) -> Result<String, std::io::Error> {
    write!(output, "{question}\n> ")?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(answer.trim().to_owned())
}

fn run_relay_client() -> Result<(), Box<dyn Error>> {
    use std::io::{Read, Write};

    const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
    let paths = AppPaths::discover()?;
    let discovery: commonkit_service::DaemonDiscovery = serde_json::from_slice(
        &std::fs::read(paths.state.join("daemon.json"))
            .map_err(|_| "relay_unavailable: CommonKit daemon discovery is unavailable")?,
    )?;
    let relay_port = discovery
        .relay_port
        .ok_or("relay_unavailable: CommonKit relay is not running")?;
    let token = commonkit_service::ControlToken::load(&paths.config.join("control.token"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let endpoint = format!("http://127.0.0.1:{relay_port}/mcp");
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    loop {
        let Some(message) = read_bounded_line(&mut input, MAX_MESSAGE_BYTES)? else {
            break;
        };
        if message.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        serde_json::from_slice::<Value>(&message)
            .map_err(|_| "relay_client_invalid_message: stdin must contain JSON-RPC lines")?;
        let mut response = client
            .post(&endpoint)
            .bearer_auth(token.expose_for_client())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(message)
            .send()
            .map_err(|_| "relay_unavailable: authenticated relay request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "relay_unavailable: relay returned HTTP {}",
                response.status().as_u16()
            )
            .into());
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MESSAGE_BYTES as u64)
        {
            return Err("relay_client_response_too_large".into());
        }
        let mut body = Vec::new();
        response
            .by_ref()
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_to_end(&mut body)?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err("relay_client_response_too_large".into());
        }
        output.write_all(&body)?;
        if !body.ends_with(b"\n") {
            output.write_all(b"\n")?;
        }
        output.flush()?;
    }
    Ok(())
}

fn read_bounded_line(
    input: &mut impl std::io::BufRead,
    maximum: usize,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let mut line = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(take) > maximum {
            return Err("relay_client_message_too_large".into());
        }
        line.extend_from_slice(&available[..take]);
        input.consume(take);
        if line.ends_with(b"\n") {
            return Ok(Some(line));
        }
    }
}

fn run_daemon_lifecycle(command: DaemonCommand) -> Result<(), Box<dyn Error>> {
    if let DaemonCommand::ReloadDomains { confirmed } = &command {
        if !*confirmed {
            return Err("confirmation_required: pass --confirmed to reload daemon domains".into());
        }
        return print_daemon(daemon_control(
            "POST",
            "/control/v1/domains/reload",
            Some(json!({"confirmed": true})),
            None,
        )?);
    }
    let current = std::env::current_exe()?;
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let daemon = current
        .parent()
        .ok_or("installed CLI has no parent directory")?
        .join(format!("commonkitd{extension}"));
    if !daemon.is_file() {
        return Err(format!("installed daemon is missing: {}", daemon.display()).into());
    }
    let service = commonkit_cli::daemon_lifecycle::DaemonService::discover(daemon)?;
    let status = match command {
        DaemonCommand::Install => service.install()?,
        DaemonCommand::Start => service.start()?,
        DaemonCommand::Status => service.status()?,
        DaemonCommand::Restart => service.restart()?,
        DaemonCommand::ReloadDomains { .. } => unreachable!("handled above"),
        DaemonCommand::Uninstall => service.uninstall()?,
    };
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

fn required_path(value: Option<PathBuf>, flag: &str) -> Result<PathBuf, Box<dyn Error>> {
    value.ok_or_else(|| format!("{flag} is required for the selected provider").into())
}

fn require_pin(found: Option<&str>, expected: &str, provider: &str) -> Result<(), Box<dyn Error>> {
    match found {
        Some(version) if version == expected => Ok(()),
        _ => Err(
            format!("{provider} provider version must be explicitly pinned to {expected}").into(),
        ),
    }
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
            allow_mock_backend,
            model,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            if backend == "mock" && !allow_mock_backend {
                return Err("the mock SkillOpt backend is development-only; pass --allow-mock-backend explicitly or select a production backend".into());
            }
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
                    "allowMockBackend": allow_mock_backend,
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
        SkillsCommand::CanaryApply {
            deployment,
            run_id,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/canary/apply",
                Some(json!({
                    "confirmed": true,
                    "confirmationId": nonce("skill-canary-apply"),
                    "runId": run_id,
                    "deployment": read_json_file::<serde_json::Value>(&deployment)?,
                })),
                None,
            )?)?;
        }
        SkillsCommand::CanaryRollback {
            run_id,
            deployment_receipt_id,
            confirmed,
        } => {
            require_skill_confirmation(confirmed)?;
            print_daemon(daemon_control(
                "POST",
                "/control/v1/skills/canary/rollback",
                Some(json!({
                    "confirmed": true,
                    "confirmationId": nonce("skill-canary-rollback"),
                    "runId": run_id,
                    "deploymentReceiptId": deployment_receipt_id,
                })),
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
        CredentialCommand::Plan { destination_ids } => daemon_control(
            "POST",
            "/control/v1/credentials/plan",
            Some(json!({"destinationIds": destination_ids})),
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
            plan_id,
            confirmed: true,
        } => daemon_control(
            "POST",
            "/control/v1/credentials/apply",
            Some(json!({
                "planId": plan_id,
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

fn require_confirmation(confirmed: bool) -> Result<(), Box<dyn Error>> {
    if confirmed {
        Ok(())
    } else {
        Err("confirmation_required: pass --confirmed after reviewing the operation".into())
    }
}

fn run_schedule(command: ScheduleCommand) -> Result<(), Box<dyn Error>> {
    let value = match command {
        ScheduleCommand::Status => daemon_control("GET", "/control/v1/schedule", None, None)?,
        ScheduleCommand::Enable {
            interval_seconds,
            confirmed,
        } => {
            require_confirmation(confirmed)?;
            daemon_control(
                "POST",
                "/control/v1/schedule",
                Some(json!({
                    "enabled": true,
                    "intervalSeconds": interval_seconds,
                    "confirmed": true,
                    "confirmationId": nonce("schedule-enable")
                })),
                None,
            )?
        }
        ScheduleCommand::Disable { confirmed } => {
            require_confirmation(confirmed)?;
            daemon_control(
                "POST",
                "/control/v1/schedule",
                Some(json!({
                    "enabled": false,
                    "confirmed": true,
                    "confirmationId": nonce("schedule-disable")
                })),
                None,
            )?
        }
    };
    print_daemon(value)
}

fn run_snapshots(command: SnapshotCommand) -> Result<(), Box<dyn Error>> {
    let value = match command {
        SnapshotCommand::List => daemon_control("GET", "/control/v1/snapshots", None, None)?,
        SnapshotCommand::Create {
            database_id,
            confirmed,
        } => {
            require_confirmation(confirmed)?;
            daemon_control(
                "POST",
                "/control/v1/snapshots",
                Some(json!({
                    "databaseId": database_id,
                    "confirmed": true,
                    "confirmationId": nonce("snapshot-create")
                })),
                None,
            )?
        }
        SnapshotCommand::Restore {
            snapshot_id,
            confirmed,
        } => {
            require_confirmation(confirmed)?;
            daemon_control(
                "POST",
                "/control/v1/snapshots/restore",
                Some(json!({
                    "snapshotId": snapshot_id,
                    "confirmed": true,
                    "confirmationId": nonce("snapshot-restore")
                })),
                None,
            )?
        }
        SnapshotCommand::Promote {
            database_id,
            target_id,
            confirmed,
        } => {
            require_confirmation(confirmed)?;
            daemon_control(
                "POST",
                "/control/v1/snapshots/promote",
                Some(json!({
                    "databaseId": database_id,
                    "targetId": target_id,
                    "confirmed": true,
                    "confirmationId": nonce("snapshot-promote")
                })),
                None,
            )?
        }
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
    let result = compose_layers(&layers, &v1_merge_rules())?;
    let organization = layers
        .iter()
        .find(|layer| layer.kind == LayerKind::OrganizationPolicy)
        .and_then(|layer| layer.spec.get("securityPolicy"))
        .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
        .transpose()?
        .unwrap_or_default();
    let effective = result
        .spec
        .get("securityPolicy")
        .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
        .transpose()?
        .unwrap_or_default();
    enforce_policy_floor(&organization, &effective)?;
    Ok(result)
}

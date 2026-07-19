//! Durable, review-gated optimization lifecycle for Git-owned agent skills.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use commonkit_adapters::{
    ArtifactError, ArtifactStore, ContentReference, ContentSensitivity as ArtifactSensitivity,
};
use commonkit_contracts::{
    CandidateState, ContentSensitivity, ContractError, EvaluationReceipt, EvidenceConsent,
    EvidenceEnvelope, EvidenceRetention, EvidenceSourceKind, GitRevision, ProviderLock,
    ProviderSource, SchemaVersion, Sha256Digest, SkillCandidate, SkillEvaluationSuite,
    SkillOptimizationManifest, StableId, assert_no_embedded_secrets, canonical_json,
    digest_domain_json,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

pub trait SkillOptimizer {
    fn optimize(
        &self,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
        current_skill: &[u8],
    ) -> Result<OptimizerOutput, SkillError>;
}

#[derive(Debug, Clone)]
pub struct OptimizerOutput {
    pub candidate: Vec<u8>,
    pub evaluation: EvaluationReceipt,
    pub history: Vec<u8>,
    pub policy_passed: bool,
}

#[derive(Debug, Clone)]
pub struct EvidenceImport {
    pub skill_id: StableId,
    pub source_kind: EvidenceSourceKind,
    pub consent: EvidenceConsent,
    pub retention: EvidenceRetention,
    pub created_at_unix_ms: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidencePreview {
    pub redacted: String,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceRecord {
    envelope: EvidenceEnvelope,
    content: ContentReference,
    report: ContentReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInventoryEntry {
    pub id: StableId,
    pub source_path: commonkit_contracts::PortableSourcePath,
    pub source_digest: Sha256Digest,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillOpportunity {
    pub skill_id: StableId,
    pub reviewed_evidence_count: u64,
    pub eligible: bool,
}

#[derive(Debug, Clone)]
pub struct FakeOptimizer {
    candidate: Vec<u8>,
    evaluation: EvaluationReceipt,
}

pub const SKILLOPT_ADAPTER_CONTRACT: &str = "skillopt-sleep-v1";
pub const SKILLOPT_SUPPORTED_VERSION: &str = "0.2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOptBackend {
    Mock,
    Claude,
    Codex,
    Handoff,
    AzureOpenAi,
}

impl SkillOptBackend {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Handoff => "handoff",
            Self::AzureOpenAi => "azure_openai",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillOptProviderConfig {
    pub environment_root: PathBuf,
    pub provider_lock: ProviderLock,
    pub backend: SkillOptBackend,
    pub model: Option<String>,
    pub tasks_file: PathBuf,
    /// Trusted repository harness adapter. It runs only after SkillOpt exits
    /// and receives the held-out corpus separately from provider staging.
    pub harness_executable: PathBuf,
    pub harness_corpus: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub timeout_seconds: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HarnessEvaluationReport {
    schema_version: SchemaVersion,
    suite_digest: Sha256Digest,
    skill_id: StableId,
    baseline_digest: Sha256Digest,
    candidate_digest: Sha256Digest,
    policy_digest: Sha256Digest,
    policy_passed: bool,
    evaluation: EvaluationReceipt,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewedTasksBinding {
    skill_id: StableId,
    campaign_id: StableId,
    suite_digest: Sha256Digest,
    train_digest: Sha256Digest,
    validation_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCheck {
    pub provider: StableId,
    pub version: String,
    pub adapter_contract: StableId,
    pub source: ProviderSource,
    pub package_digest: Sha256Digest,
    /// Digest of the installed provider tree, excluding this check file.
    pub installed_content_digest: Sha256Digest,
    pub capabilities: BTreeSet<StableId>,
    pub compatible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeApproval {
    pub approver: StableId,
    pub approved_at_unix_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUpgradePlan {
    pub id: Sha256Digest,
    pub provider_lock: ProviderLock,
    pub fixture_skill: Vec<u8>,
    pub reviewed_tasks: Vec<u8>,
    pub fixture_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderUpgradeReport {
    pub id: Sha256Digest,
    pub plan_id: Sha256Digest,
    pub provider_lock: ProviderLock,
    pub environment_root: PathBuf,
    pub requirements_digest: Sha256Digest,
    pub baseline_basis_points: u32,
    pub candidate_basis_points: u32,
    pub source_unchanged: bool,
    pub compatible: bool,
}

pub struct SkillOptProviderManager {
    uv: PathBuf,
    root: PathBuf,
    harness: Option<(PathBuf, PathBuf)>,
}

impl SkillOptProviderManager {
    pub fn open(uv: impl AsRef<Path>, root: impl AsRef<Path>) -> Result<Self, SkillError> {
        let uv = uv.as_ref().canonicalize()?;
        if !fs::symlink_metadata(&uv)?.is_file() {
            return Err(SkillError::InvalidProviderManager);
        }
        fs::create_dir_all(root.as_ref())?;
        set_private_directory(root.as_ref())?;
        Ok(Self {
            uv,
            root: root.as_ref().canonicalize()?,
            harness: None,
        })
    }

    pub fn with_harness(
        mut self,
        executable: impl AsRef<Path>,
        corpus: impl AsRef<Path>,
    ) -> Result<Self, SkillError> {
        let executable = executable.as_ref().canonicalize()?;
        let corpus = corpus.as_ref().canonicalize()?;
        if !executable.is_file() || !corpus.is_dir() {
            return Err(SkillError::InvalidHarness);
        }
        self.harness = Some((executable, corpus));
        Ok(self)
    }

    pub fn plan_upgrade(
        &self,
        provider_lock: ProviderLock,
        fixtures: impl AsRef<Path>,
    ) -> Result<ProviderUpgradePlan, SkillError> {
        validate_provider_lock(&provider_lock)?;
        let fixture_skill = read_bounded(&fixtures.as_ref().join("SKILL.md"), 256 * 1024)?;
        let reviewed_tasks = read_bounded(
            &fixtures.as_ref().join("reviewed-tasks.json"),
            8 * 1024 * 1024,
        )?;
        let temporary = create_private_staging()?;
        let tasks_path = temporary.join("tasks.json");
        fs::write(&tasks_path, &reviewed_tasks)?;
        let mut suite = provider_fixture_suite()?;
        let (harness_executable, harness_corpus) =
            self.harness.as_ref().ok_or(SkillError::InvalidHarness)?;
        suite.harness = measure_harness_lock(
            suite.harness.kind.clone(),
            suite.harness.version.clone(),
            harness_executable,
            harness_corpus,
        )?;
        let manifest = provider_fixture_manifest(
            provider_lock.clone(),
            digest_bytes(&fixture_skill)?,
            &suite,
        )?;
        let validation = validate_reviewed_tasks(&tasks_path, &manifest, &suite);
        let _ = fs::remove_dir_all(temporary);
        validation?;
        let fixture_digest = digest_domain_json(
            "commonkit.skillopt-provider-fixtures.v1",
            &(
                digest_bytes(&fixture_skill)?,
                digest_bytes(&reviewed_tasks)?,
            ),
        )?;
        let draft = (&provider_lock, &fixture_digest);
        Ok(ProviderUpgradePlan {
            id: digest_domain_json("commonkit.skillopt-provider-upgrade-plan.v1", &draft)?,
            provider_lock,
            fixture_skill,
            reviewed_tasks,
            fixture_digest,
        })
    }

    pub fn execute_upgrade(
        &self,
        plan: &ProviderUpgradePlan,
    ) -> Result<ProviderUpgradeReport, SkillError> {
        validate_upgrade_plan(plan)?;
        let upgrade_root = self
            .root
            .join(format!(".upgrade-{}", &digest_filename(&plan.id)[..16]));
        if upgrade_root.exists() {
            return Err(SkillError::UpgradeAlreadyExists);
        }
        fs::create_dir(&upgrade_root)?;
        set_private_directory(&upgrade_root)?;
        let result = self.execute_upgrade_in(&upgrade_root, plan);
        if result.is_err() {
            let _ = fs::remove_dir_all(&upgrade_root);
        }
        result
    }

    fn execute_upgrade_in(
        &self,
        upgrade_root: &Path,
        plan: &ProviderUpgradePlan,
    ) -> Result<ProviderUpgradeReport, SkillError> {
        let environment = upgrade_root.join("environment");
        run_uv(
            &self.uv,
            upgrade_root,
            ["venv".into(), environment.as_os_str().into()],
        )?;
        let input = upgrade_root.join("requirements.in");
        fs::write(
            &input,
            format!("skillopt=={}\n", plan.provider_lock.version),
        )?;
        let requirements = upgrade_root.join("requirements.lock");
        run_uv(
            &self.uv,
            upgrade_root,
            [
                "pip".into(),
                "compile".into(),
                input.as_os_str().into(),
                "--generate-hashes".into(),
                "--output-file".into(),
                requirements.as_os_str().into(),
            ],
        )?;
        let requirement_bytes = read_bounded(&requirements, 8 * 1024 * 1024)?;
        let expected_hash = plan
            .provider_lock
            .package_digest
            .as_str()
            .trim_start_matches("sha256:");
        let requirement_text = std::str::from_utf8(&requirement_bytes)
            .map_err(|_| SkillError::ProviderRequirementsMismatch)?;
        if !direct_requirement_has_hash(
            requirement_text,
            "skillopt",
            &plan.provider_lock.version,
            expected_hash,
        ) {
            return Err(SkillError::ProviderRequirementsMismatch);
        }
        run_uv(
            &self.uv,
            upgrade_root,
            [
                "pip".into(),
                "sync".into(),
                "--python".into(),
                environment.join("bin/python").as_os_str().into(),
                requirements.as_os_str().into(),
                "--require-hashes".into(),
            ],
        )?;
        let marker = provider_check_from_lock(
            &plan.provider_lock,
            measure_provider_installation(&environment)?,
        );
        fs::write(
            environment.join("commonkit-provider-lock.json"),
            canonical_json(&marker)?,
        )?;
        check_skillopt_provider(&environment, &plan.provider_lock)?;

        let fixtures = upgrade_root.join("fixtures");
        fs::create_dir(&fixtures)?;
        let skill_path = fixtures.join("SKILL.md");
        let tasks_path = fixtures.join("reviewed-tasks.json");
        fs::write(&skill_path, &plan.fixture_skill)?;
        fs::write(&tasks_path, &plan.reviewed_tasks)?;
        let mut suite = provider_fixture_suite()?;
        let (harness_executable, harness_corpus) =
            self.harness.as_ref().ok_or(SkillError::InvalidHarness)?;
        suite.harness = measure_harness_lock(
            suite.harness.kind.clone(),
            suite.harness.version.clone(),
            harness_executable,
            harness_corpus,
        )?;
        let source_digest = digest_bytes(&plan.fixture_skill)?;
        let manifest =
            provider_fixture_manifest(plan.provider_lock.clone(), source_digest, &suite)?;
        let optimizer = SkillOptSleepOptimizer::new(SkillOptProviderConfig {
            environment_root: environment.clone(),
            provider_lock: plan.provider_lock.clone(),
            backend: SkillOptBackend::Mock,
            model: None,
            tasks_file: tasks_path,
            environment: BTreeMap::new(),
            timeout_seconds: 60,
            harness_executable: harness_executable.clone(),
            harness_corpus: harness_corpus.clone(),
        })?;
        let output = optimizer.optimize(&manifest, &suite, &plan.fixture_skill)?;
        let source_unchanged = fs::read(&skill_path)? == plan.fixture_skill;
        if !source_unchanged {
            return Err(SkillError::ProviderMutatedInput);
        }
        let final_environment = self.root.join(format!(
            "{}-{}",
            plan.provider_lock.version,
            &digest_filename(&plan.provider_lock.package_digest)[..16]
        ));
        if final_environment.exists() {
            return Err(SkillError::ProviderEnvironmentAlreadyInstalled);
        }
        fs::rename(&environment, &final_environment)?;
        sync_parent(&final_environment)?;
        let draft = ProviderUpgradeReportDraft {
            plan_id: plan.id.clone(),
            provider_lock: plan.provider_lock.clone(),
            environment_root: final_environment.clone(),
            requirements_digest: digest_bytes(&requirement_bytes)?,
            baseline_basis_points: output.evaluation.baseline_basis_points,
            candidate_basis_points: output.evaluation.candidate_basis_points,
            source_unchanged,
            compatible: true,
        };
        let report = ProviderUpgradeReport {
            id: digest_domain_json("commonkit.skillopt-provider-upgrade-report.v1", &draft)?,
            plan_id: draft.plan_id,
            provider_lock: draft.provider_lock,
            environment_root: draft.environment_root,
            requirements_digest: draft.requirements_digest,
            baseline_basis_points: draft.baseline_basis_points,
            candidate_basis_points: draft.candidate_basis_points,
            source_unchanged: draft.source_unchanged,
            compatible: draft.compatible,
        };
        fs::write(
            upgrade_root.join("upgrade-report.json"),
            canonical_json(&report)?,
        )?;
        Ok(report)
    }

    pub fn activate_upgrade(
        &self,
        report: &ProviderUpgradeReport,
        approval: UpgradeApproval,
        active_lock: impl AsRef<Path>,
    ) -> Result<(), SkillError> {
        validate_upgrade_report(report)?;
        validate_approval(&Approval {
            approver: approval.approver,
            approved_at_unix_ms: approval.approved_at_unix_ms,
            reason: approval.reason,
        })?;
        check_skillopt_provider(&report.environment_root, &report.provider_lock)?;
        atomic_write(
            active_lock.as_ref(),
            &canonical_json(&report.provider_lock)?,
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUpgradeReportDraft {
    plan_id: Sha256Digest,
    provider_lock: ProviderLock,
    environment_root: PathBuf,
    requirements_digest: Sha256Digest,
    baseline_basis_points: u32,
    candidate_basis_points: u32,
    source_unchanged: bool,
    compatible: bool,
}

pub struct SkillOptSleepOptimizer {
    config: SkillOptProviderConfig,
    executable: PathBuf,
}

impl SkillOptSleepOptimizer {
    pub fn new(mut config: SkillOptProviderConfig) -> Result<Self, SkillError> {
        validate_provider_lock(&config.provider_lock)?;
        if config.timeout_seconds == 0 || config.timeout_seconds > 86_400 {
            return Err(SkillError::InvalidProviderConfiguration);
        }
        validate_provider_environment(&config.environment)?;
        validate_reviewed_tasks_shape(&config.tasks_file)?;
        let harness = fs::symlink_metadata(&config.harness_executable)?;
        let corpus = fs::symlink_metadata(&config.harness_corpus)?;
        if !harness.is_file()
            || harness.file_type().is_symlink()
            || !corpus.is_dir()
            || corpus.file_type().is_symlink()
        {
            return Err(SkillError::InvalidHarness);
        }
        let root = config.environment_root.canonicalize()?;
        config.environment_root = root.clone();
        config.harness_executable = config.harness_executable.canonicalize()?;
        config.harness_corpus = config.harness_corpus.canonicalize()?;
        let executable = root.join("bin/skillopt-sleep");
        let python = root.join("bin/python");
        for path in [&executable, &python] {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(SkillError::InvalidProviderEnvironment);
            }
        }
        let optimizer = Self { config, executable };
        optimizer.check()?;
        Ok(optimizer)
    }

    pub fn check(&self) -> Result<ProviderCheck, SkillError> {
        check_skillopt_provider(&self.config.environment_root, &self.config.provider_lock)
    }

    fn run_external(
        &self,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
        current_skill: &[u8],
    ) -> Result<OptimizerOutput, SkillError> {
        if manifest.provider != self.config.provider_lock {
            return Err(SkillError::ProviderLockMismatch);
        }
        validate_reviewed_tasks(&self.config.tasks_file, manifest, suite)?;
        self.check()?;
        let measured_harness = measure_harness_lock(
            suite.harness.kind.clone(),
            suite.harness.version.clone(),
            &self.config.harness_executable,
            &self.config.harness_corpus,
        )?;
        if measured_harness != suite.harness {
            return Err(SkillError::HarnessIntegrityMismatch);
        }
        let staging = create_private_staging()?;
        let result = self.run_in_staging(&staging, manifest, suite, current_skill);
        let _ = fs::remove_dir_all(&staging);
        result
    }

    fn run_in_staging(
        &self,
        staging: &Path,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
        current_skill: &[u8],
    ) -> Result<OptimizerOutput, SkillError> {
        let input = staging.join("input");
        fs::create_dir_all(&input)?;
        set_private_directory(&input)?;
        let skill_path = input.join("SKILL.md");
        fs::write(&skill_path, current_skill)?;
        let tasks_path = input.join("reviewed-tasks.json");
        fs::copy(&self.config.tasks_file, &tasks_path)?;
        let provider_suite_path = input.join("suite-manifest.json");
        fs::write(
            &provider_suite_path,
            canonical_json(&serde_json::json!({
                "schemaVersion": suite.schema_version,
                "id": suite.id,
                "skillId": suite.skill_id,
                "train": suite.train,
                "validation": suite.validation,
                "rubricDigest": suite.rubric_digest,
                "harness": suite.harness,
                "metric": {
                    "id": suite.metric.id,
                    "minimumImprovementBasisPoints": suite.metric.minimum_improvement_basis_points,
                }
            }))?,
        )?;
        let stdout_path = staging.join("stdout.json");
        let stderr_path = staging.join("stderr.log");
        let stdout = private_output_file(&stdout_path)?;
        let stderr = private_output_file(&stderr_path)?;
        let home = staging.join("home");
        fs::create_dir_all(&home)?;
        set_private_directory(&home)?;

        let mut command =
            isolated_command(&self.executable, staging, &[&self.config.environment_root])?;
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_CACHE_HOME", home.join(".cache"));
        for (name, value) in &self.config.environment {
            command.env(name, value);
        }
        command
            .arg("run")
            .arg("--project")
            .arg(staging)
            .arg("--backend")
            .arg(self.config.backend.as_arg())
            .arg("--edit-budget")
            .arg(manifest.limits.maximum_edits.to_string())
            .arg("--max-tasks")
            .arg(manifest.limits.maximum_cases.to_string())
            .arg("--target-skill-path")
            .arg(&skill_path)
            .arg("--tasks-file")
            .arg(&tasks_path)
            .arg("--json");
        if let Some(model) = &self.config.model {
            command.arg("--model").arg(model);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;
        let provider_group = child.id();
        let deadline = Instant::now() + Duration::from_secs(self.config.timeout_seconds.into());
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if file_exceeds(&stdout_path, 2 * 1024 * 1024)?
                || file_exceeds(&stderr_path, 2 * 1024 * 1024)?
            {
                terminate_process_group(&mut child, provider_group);
                return Err(SkillError::ProviderOutputIncompatible);
            }
            if Instant::now() >= deadline {
                terminate_process_group(&mut child, provider_group);
                return Err(SkillError::ProviderTimedOut);
            }
            thread::sleep(Duration::from_millis(25));
        };
        // The direct provider may exit while daemonized descendants remain.
        // Terminate and reap the entire isolated job before any held-out state
        // or trusted harness output path is created.
        terminate_process_group(&mut child, provider_group);
        if !status.success() {
            return Err(SkillError::ProviderExecutionFailed);
        }
        if read_ordinary_file(&skill_path)? != current_skill {
            return Err(SkillError::ProviderMutatedInput);
        }
        let stdout = read_bounded(&stdout_path, 2 * 1024 * 1024)?;
        let report: SkillOptJsonReport =
            serde_json::from_slice(&stdout).map_err(|_| SkillError::ProviderOutputIncompatible)?;
        if !report.accepted
            || !matches!(report.gate_action.as_str(), "accept" | "accept_new_best")
            || !report.no_edits_reason.is_empty()
            || !report.adopted.is_empty()
            || report.night == 0
            || report.n_tasks == 0
            || report.n_sessions > manifest.limits.maximum_cases.into()
            || report.n_accepted_edits as usize != report.edits.len()
            || report.n_accepted_edits == 0
            || report.n_rejected_edits as usize != report.rejected_edits.len()
            || report.n_accepted_edits > manifest.limits.maximum_edits.into()
            || report.notes.iter().any(|note| note.len() > 10_000)
            || report.tasks_reviewed != Some(true)
            || report.tasks_file.as_deref().is_none_or(str::is_empty)
        {
            return Err(SkillError::ProviderCandidateRejected);
        }
        let provider_baseline = score_basis_points(report.baseline)?;
        let provider_candidate = score_basis_points(report.candidate)?;
        if provider_candidate <= provider_baseline {
            return Err(SkillError::ProviderCandidateRejected);
        }
        let staging_dir = PathBuf::from(&report.staging_dir).canonicalize()?;
        if !staging_dir.starts_with(staging) {
            return Err(SkillError::ProviderOutputEscapedStaging);
        }
        let staged_manifest: SkillOptStagedManifest =
            read_json(&staging_dir.join("manifest.json"))?;
        if !staged_manifest.accepted
            || !staged_manifest.has_skill
            || staged_manifest.has_memory
            || !staged_manifest.live_memory_path.is_empty()
            || Path::new(&staged_manifest.live_skill_path) != skill_path
        {
            return Err(SkillError::ProviderOutputIncompatible);
        }
        let candidate = read_bounded(&staging_dir.join("proposed_SKILL.md"), 256 * 1024)?;
        validate_candidate_content(&candidate)?;
        // Provider descendants inherit the provider sandbox. Keep held-out
        // identities in a separate root that was never provider-readable.
        let harness_private = PrivateStaging::new("commonkit-skill-harness")?;
        let harness_suite_path = harness_private.path().join("suite-manifest.json");
        fs::write(&harness_suite_path, canonical_json(suite)?)?;
        let harness_stdout = harness_private.path().join("evaluation.json");
        let harness_stderr = harness_private.path().join("stderr.log");
        let mut harness = isolated_command(
            &self.config.harness_executable,
            staging,
            &[&self.config.harness_corpus, harness_private.path()],
        )?;
        harness
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg("evaluate")
            .arg("--baseline")
            .arg(&skill_path)
            .arg("--candidate")
            .arg(staging_dir.join("proposed_SKILL.md"))
            .arg("--suite-manifest")
            .arg(&harness_suite_path)
            .arg("--corpus")
            .arg(&self.config.harness_corpus)
            .arg("--suite-digest")
            .arg(suite.digest()?.as_str())
            .arg("--skill-id")
            .arg(manifest.skill.id.as_str())
            .arg("--baseline-digest")
            .arg(digest_bytes(current_skill)?.as_str())
            .arg("--candidate-digest")
            .arg(digest_bytes(&candidate)?.as_str())
            .arg("--policy-digest")
            .arg(manifest.policy_digest.as_str())
            .arg("--harness-environment-digest")
            .arg(suite.harness.environment_digest.as_str())
            .arg("--scorer-digest")
            .arg(
                digest_bytes(&read_ordinary_file(
                    &self.config.harness_corpus.join("scorer"),
                )?)?
                .as_str(),
            )
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::from(private_output_file(&harness_stdout)?))
            .stderr(Stdio::from(private_output_file(&harness_stderr)?));
        run_bounded_child(
            &mut harness,
            &harness_stdout,
            &harness_stderr,
            self.config.timeout_seconds,
        )?;
        if staging.join("held-out-leaked").exists() {
            return Err(SkillError::ProviderIsolationBreached);
        }
        let evaluation_bytes = read_bounded(&harness_stdout, 2 * 1024 * 1024)?;
        let harness_report: HarnessEvaluationReport = serde_json::from_slice(&evaluation_bytes)
            .map_err(|_| SkillError::HarnessOutputIncompatible)?;
        if harness_report.schema_version != SchemaVersion(1)
            || harness_report.suite_digest != suite.digest()?
            || harness_report.skill_id != manifest.skill.id
            || harness_report.baseline_digest != digest_bytes(current_skill)?
            || harness_report.candidate_digest != digest_bytes(&candidate)?
            || harness_report.policy_digest != manifest.policy_digest
            || harness_report.evaluation.harness != suite.harness
            || harness_report.evaluation.scorer_digest
                != digest_bytes(&read_ordinary_file(
                    &self.config.harness_corpus.join("scorer"),
                )?)?
        {
            return Err(SkillError::HarnessOutputIncompatible);
        }
        Ok(OptimizerOutput {
            candidate,
            evaluation: harness_report.evaluation,
            history: stdout,
            policy_passed: harness_report.policy_passed,
        })
    }
}

pub fn check_skillopt_provider(
    environment_root: &Path,
    lock: &ProviderLock,
) -> Result<ProviderCheck, SkillError> {
    validate_provider_lock(lock)?;
    let environment_root = environment_root.canonicalize()?;
    let python = environment_root.join("bin/python");
    let executable = environment_root.join("bin/skillopt-sleep");
    for path in [&python, &executable] {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SkillError::InvalidProviderEnvironment);
        }
    }
    let marker_path = environment_root.join("commonkit-provider-lock.json");
    let marker: ProviderCheck = read_json(&marker_path)?;
    let installed_content_digest = measure_provider_installation(&environment_root)?;
    let version = Command::new(&python)
        .env_clear()
        .arg("-c")
        .arg("import importlib.metadata; print(importlib.metadata.version('skillopt'))")
        .output()?;
    if !version.status.success() {
        return Err(SkillError::ProviderVersionProbeFailed);
    }
    let installed_version = String::from_utf8(version.stdout)
        .map_err(|_| SkillError::ProviderVersionProbeFailed)?
        .trim()
        .to_string();
    let expected = ProviderCheck {
        provider: lock.id.clone(),
        version: lock.version.clone(),
        adapter_contract: lock.adapter_contract.clone(),
        source: lock.source,
        package_digest: lock.package_digest.clone(),
        installed_content_digest,
        capabilities: lock.capabilities.clone(),
        compatible: true,
    };
    if marker != expected || installed_version != lock.version {
        return Err(SkillError::UnsupportedSkillOptProvider);
    }
    Ok(expected)
}

impl SkillOptimizer for SkillOptSleepOptimizer {
    fn optimize(
        &self,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
        current_skill: &[u8],
    ) -> Result<OptimizerOutput, SkillError> {
        self.run_external(manifest, suite, current_skill)
    }
}

impl commonkit_reconcile::SkillPromotionAuthority for SkillEngine {
    fn authenticate(
        &self,
        promotion_receipt_id: &Sha256Digest,
    ) -> Result<
        commonkit_reconcile::AuthenticatedSkillPromotion,
        commonkit_reconcile::SkillDeploymentError,
    > {
        let authenticate =
            || -> Result<commonkit_reconcile::AuthenticatedSkillPromotion, SkillError> {
                let receipt: PromotionReceipt =
                    read_json(&self.receipt_path(promotion_receipt_id))?;
                let receipt_draft = PromotionReceiptDraft {
                    schema_version: receipt.schema_version,
                    plan_id: receipt.plan_id.clone(),
                    candidate_id: receipt.candidate_id.clone(),
                    source_path: receipt.source_path.clone(),
                    before: receipt.before.clone(),
                    after: receipt.after.clone(),
                    state: receipt.state,
                };
                if receipt.id != *promotion_receipt_id
                    || receipt.state != PromotionState::Promoted
                    || digest_domain_json("commonkit.skill-promotion-receipt.v1", &receipt_draft)?
                        != receipt.id
                {
                    return Err(SkillError::PromotionReceiptMismatch);
                }
                let candidate = self.load_candidate(&receipt.candidate_id)?;
                let plan: PromotionPlan = read_json(&self.plan_path(&receipt.plan_id))?;
                validate_promotion_plan(&plan)?;
                if plan.id != receipt.plan_id
                    || plan.candidate_id != candidate.candidate.id
                    || plan.candidate_digest != candidate.candidate.candidate_digest
                    || receipt.after.digest != candidate.candidate.candidate_digest
                    || receipt.source_path != candidate.source_path
                    || candidate.candidate.state != CandidateState::Approvable
                    || self.current_policy_digest()? != candidate.policy_digest
                    || digest_bytes(&read_ordinary_file(
                        &self.resolve_source(receipt.source_path.as_str())?,
                    )?)? != receipt.after.digest
                {
                    return Err(SkillError::PromotionReceiptMismatch);
                }
                Ok(commonkit_reconcile::AuthenticatedSkillPromotion {
                    candidate_id: candidate.candidate.id,
                    candidate_digest: candidate.candidate.candidate_digest,
                    promotion_receipt_id: receipt.id,
                    promoted_source_digest: receipt.after.digest,
                    policy_digest: candidate.policy_digest,
                })
            };
        authenticate()
            .map_err(|_| commonkit_reconcile::SkillDeploymentError::PromotionAuthorityMismatch)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillOptJsonReport {
    night: u64,
    accepted: bool,
    gate_action: String,
    no_edits_reason: String,
    baseline: f64,
    candidate: f64,
    n_tasks: u64,
    n_sessions: u64,
    n_accepted_edits: u64,
    n_rejected_edits: u64,
    edits: Vec<Value>,
    rejected_edits: Vec<Value>,
    notes: Vec<String>,
    staging_dir: String,
    adopted: Vec<String>,
    #[serde(default)]
    tasks_file: Option<String>,
    #[serde(default)]
    tasks_reviewed: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillOptStagedManifest {
    live_skill_path: String,
    live_memory_path: String,
    has_skill: bool,
    has_memory: bool,
    accepted: bool,
}

impl FakeOptimizer {
    pub fn new(candidate: Vec<u8>, evaluation: EvaluationReceipt) -> Self {
        Self {
            candidate,
            evaluation,
        }
    }
}

impl SkillOptimizer for FakeOptimizer {
    fn optimize(
        &self,
        _manifest: &SkillOptimizationManifest,
        _suite: &SkillEvaluationSuite,
        _current_skill: &[u8],
    ) -> Result<OptimizerOutput, SkillError> {
        Ok(OptimizerOutput {
            candidate: self.candidate.clone(),
            evaluation: self.evaluation.clone(),
            history: br#"{"provider":"fake","deterministic":true}"#.to_vec(),
            policy_passed: true,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Approval {
    pub approver: StableId,
    pub approved_at_unix_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionPlan {
    pub schema_version: SchemaVersion,
    pub id: Sha256Digest,
    pub candidate_id: StableId,
    pub candidate_digest: Sha256Digest,
    pub parent_skill_digest: Sha256Digest,
    pub source_path: commonkit_contracts::PortableSourcePath,
    pub repository_revision: GitRevision,
    pub policy_digest: Sha256Digest,
    pub suite_digest: Sha256Digest,
    pub approval: Approval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
    Promoted,
    RolledBack,
}

impl std::fmt::Display for PromotionState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Promoted => "promoted",
            Self::RolledBack => "rolled_back",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionReceipt {
    pub schema_version: SchemaVersion,
    pub id: Sha256Digest,
    pub plan_id: Sha256Digest,
    pub candidate_id: StableId,
    pub source_path: commonkit_contracts::PortableSourcePath,
    pub before: ContentReference,
    pub after: ContentReference,
    pub state: PromotionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CandidateRecord {
    candidate: SkillCandidate,
    source_path: commonkit_contracts::PortableSourcePath,
    repository_revision: GitRevision,
    policy_digest: Sha256Digest,
    suite_digest: Sha256Digest,
    candidate_content: ContentReference,
    parent_content: ContentReference,
    patch: ContentReference,
    history: ContentReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    Discovery,
    CandidateGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSchedule {
    pub kind: ScheduleKind,
    pub enabled: bool,
    pub interval_seconds: u64,
    pub maximum_cost_micros_per_period: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillScheduleStatus {
    pub schedule: SkillSchedule,
    pub spent_cost_micros: u64,
    pub claimed_run_ids: BTreeSet<StableId>,
}

/// Repository-scoped summary of Claude SessionEnd markers. It deliberately
/// contains no transcript content or unrelated repository paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionActivityStatus {
    pub has_new_activity: bool,
    pub marker_count: u64,
    pub latest_timestamp: Option<String>,
}

pub struct SkillEngine {
    repository: PathBuf,
    state: PathBuf,
    artifacts: ArtifactStore,
}

impl SkillEngine {
    pub fn open(repository: impl AsRef<Path>, state: impl AsRef<Path>) -> Result<Self, SkillError> {
        fs::create_dir_all(repository.as_ref())?;
        fs::create_dir_all(state.as_ref())?;
        set_private_directory(state.as_ref())?;
        let repository = repository.as_ref().canonicalize()?;
        let state = state.as_ref().canonicalize()?;
        for directory in ["candidates", "plans", "receipts", "events", "evidence"] {
            let path = state.join(directory);
            fs::create_dir_all(&path)?;
            set_private_directory(&path)?;
        }
        let artifacts = ArtifactStore::open(state.join("artifacts"))?;
        Ok(Self {
            repository,
            state,
            artifacts,
        })
    }

    pub fn configure_schedule(
        &self,
        schedule: SkillSchedule,
    ) -> Result<SkillScheduleStatus, SkillError> {
        if schedule.interval_seconds == 0
            || (schedule.kind == ScheduleKind::CandidateGeneration
                && schedule.maximum_cost_micros_per_period == 0)
        {
            return Err(SkillError::InvalidSchedule);
        }
        let status = SkillScheduleStatus {
            schedule,
            spent_cost_micros: 0,
            claimed_run_ids: BTreeSet::new(),
        };
        atomic_write(&self.state.join("schedule.json"), &canonical_json(&status)?)?;
        Ok(status)
    }

    /// Reads the inexpensive Claude SessionEnd marker protocol
    /// (`<UTC-RFC3339>\t<absolute-project-path>`). It never opens transcripts and
    /// cannot start an optimization run.
    pub fn claude_activity_since(
        &self,
        marker_log: impl AsRef<Path>,
        since_timestamp: &str,
    ) -> Result<SessionActivityStatus, SkillError> {
        let marker_log = marker_log.as_ref();
        let metadata = fs::symlink_metadata(marker_log)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SkillError::InvalidActivityMarker);
        }
        let content =
            fs::read_to_string(marker_log).map_err(|_| SkillError::InvalidActivityMarker)?;
        let mut marker_count = 0_u64;
        validate_marker_timestamp(since_timestamp)?;
        let mut latest_timestamp: Option<String> = None;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let (timestamp, project) = line
                .split_once('\t')
                .ok_or(SkillError::InvalidActivityMarker)?;
            validate_marker_timestamp(timestamp)?;
            let Ok(project) = Path::new(project).canonicalize() else {
                // SessionEnd logs are append-only and may refer to repositories
                // that have since been moved or removed.
                continue;
            };
            if project == self.repository && timestamp > since_timestamp {
                marker_count = marker_count
                    .checked_add(1)
                    .ok_or(SkillError::InvalidActivityMarker)?;
                if latest_timestamp
                    .as_deref()
                    .is_none_or(|latest| timestamp > latest)
                {
                    latest_timestamp = Some(timestamp.to_owned());
                }
            }
        }
        Ok(SessionActivityStatus {
            has_new_activity: marker_count > 0,
            marker_count,
            latest_timestamp,
        })
    }

    pub fn schedule_status(&self) -> Result<SkillScheduleStatus, SkillError> {
        let path = self.state.join("schedule.json");
        if !path.exists() {
            return Ok(SkillScheduleStatus {
                schedule: SkillSchedule {
                    kind: ScheduleKind::Discovery,
                    enabled: false,
                    interval_seconds: 86_400,
                    maximum_cost_micros_per_period: 0,
                },
                spent_cost_micros: 0,
                claimed_run_ids: BTreeSet::new(),
            });
        }
        read_json(&path)
    }

    pub fn disable_schedule(&self) -> Result<SkillScheduleStatus, SkillError> {
        let mut status = self.schedule_status()?;
        status.schedule.enabled = false;
        atomic_write(&self.state.join("schedule.json"), &canonical_json(&status)?)?;
        Ok(status)
    }

    pub fn claim_scheduled_run(
        &self,
        run_id: &StableId,
        cost_micros: u64,
    ) -> Result<bool, SkillError> {
        let mut status = self.schedule_status()?;
        if !status.schedule.enabled || status.claimed_run_ids.contains(run_id) {
            return Ok(false);
        }
        let next = status
            .spent_cost_micros
            .checked_add(cost_micros)
            .ok_or(SkillError::InvalidSchedule)?;
        if status.schedule.kind == ScheduleKind::CandidateGeneration
            && next > status.schedule.maximum_cost_micros_per_period
        {
            return Ok(false);
        }
        status.spent_cost_micros = next;
        status.claimed_run_ids.insert(run_id.clone());
        atomic_write(&self.state.join("schedule.json"), &canonical_json(&status)?)?;
        Ok(true)
    }

    pub fn optimize(
        &self,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
        optimizer: &dyn SkillOptimizer,
    ) -> Result<SkillCandidate, SkillError> {
        manifest.validate()?;
        suite.validate()?;
        if suite.skill_id != manifest.skill.id || suite.digest()? != manifest.suite_digest {
            return Err(SkillError::SuiteMismatch);
        }
        if self.repository_revision()? != manifest.repository_revision
            || self.current_policy_digest()? != manifest.policy_digest
        {
            return Err(SkillError::OptimizationContextStale);
        }
        let source = self.resolve_source(manifest.skill.source_path.as_str())?;
        let current = read_ordinary_file(&source)?;
        if digest_bytes(&current)? != manifest.skill.source_digest {
            return Err(SkillError::SourceChangedBeforeOptimization);
        }
        let output = optimizer.optimize(manifest, suite, &current)?;
        validate_candidate_content(&output.candidate)?;
        let candidate_content = self
            .artifacts
            .put(&output.candidate, ArtifactSensitivity::Portable)?;
        let parent_content = self
            .artifacts
            .put(&current, ArtifactSensitivity::Portable)?;
        let patch_bytes = render_patch(
            &current,
            &output.candidate,
            manifest.skill.source_path.as_str(),
        );
        let patch = self
            .artifacts
            .put(&patch_bytes, ArtifactSensitivity::Portable)?;
        let history = self
            .artifacts
            .put(&output.history, ArtifactSensitivity::LocalSensitive)?;
        let manifest_digest = manifest.digest()?;
        let id = candidate_id(&manifest_digest, &candidate_content.digest)?;
        let candidate = SkillCandidate {
            schema_version: SchemaVersion(1),
            id,
            manifest_digest,
            parent_skill_digest: manifest.skill.source_digest.clone(),
            provider: manifest.provider.clone(),
            optimizer: manifest.optimizer.clone(),
            target: manifest.target.clone(),
            configuration_digest: digest_domain_json(
                "commonkit.skillopt-configuration.v1",
                &manifest.limits,
            )?,
            input_digests: {
                let mut inputs = vec![
                    manifest.skill.source_digest.clone(),
                    manifest.suite_digest.clone(),
                    manifest.policy_digest.clone(),
                ];
                inputs.extend(manifest.evidence_digests.clone());
                inputs
            },
            candidate_digest: candidate_content.digest.clone(),
            candidate_bytes: candidate_content.bytes,
            candidate_sensitivity: commonkit_contracts::ContentSensitivity::Portable,
            patch_digest: patch.digest.clone(),
            evaluation: output.evaluation,
            optimization_history_digest: history.digest.clone(),
            policy_passed: output.policy_passed,
            state: if output.policy_passed {
                CandidateState::Approvable
            } else {
                CandidateState::Rejected
            },
        };
        if candidate.state == CandidateState::Approvable {
            candidate.validate_against(manifest, suite)?;
        }
        let record = CandidateRecord {
            candidate: candidate.clone(),
            source_path: manifest.skill.source_path.clone(),
            repository_revision: manifest.repository_revision.clone(),
            policy_digest: manifest.policy_digest.clone(),
            suite_digest: manifest.suite_digest.clone(),
            candidate_content,
            parent_content,
            patch,
            history,
        };
        persist_immutable(
            &self.candidate_path(&candidate.id),
            &canonical_json(&record)?,
        )?;
        self.append_event(&candidate.id, "candidate_staged", &candidate)?;
        Ok(candidate)
    }

    pub fn inventory(&self) -> Result<Vec<SkillInventoryEntry>, SkillError> {
        let mut inventory = Vec::new();
        let relative_root = ".agents/skills";
        let skills_root = self.repository.join(relative_root);
        if skills_root.exists() {
            for entry in fs::read_dir(&skills_root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let id = match StableId::parse(&name) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let source = entry.path().join("SKILL.md");
                if !source.exists() {
                    continue;
                }
                let bytes = read_ordinary_file(&source)?;
                inventory.push(SkillInventoryEntry {
                    id,
                    source_path: commonkit_contracts::PortableSourcePath::parse(format!(
                        "{relative_root}/{name}/SKILL.md"
                    ))?,
                    source_digest: digest_bytes(&bytes)?,
                    bytes: bytes
                        .len()
                        .try_into()
                        .map_err(|_| SkillError::InvalidCandidateContent)?,
                });
            }
        }
        inventory.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(inventory)
    }

    pub fn preview_evidence(&self, bytes: &[u8]) -> Result<EvidencePreview, SkillError> {
        let source = std::str::from_utf8(bytes).map_err(|_| SkillError::InvalidEvidence)?;
        let assignment = Regex::new(
            r"(?i)(?P<key>(?:api_?key|token|secret|password|authorization|credential))\s*=\s*(?P<value>[^\s]+)",
        )
        .expect("static assignment regex");
        let bearer = Regex::new(r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9+/_.=-]{8,}")
            .expect("static auth regex");
        let provider_token =
            Regex::new(r"\bsk-[A-Za-z0-9_-]{20,}\b").expect("static provider token regex");
        let private_path =
            Regex::new(r"/(?:Users|home)/[^/\s]+/[^\s]+").expect("static private path regex");
        let mut findings = Vec::new();
        let mut redacted = assignment
            .replace_all(source, |captures: &regex::Captures<'_>| {
                findings.push(format!(
                    "secret_assignment:{}",
                    &captures["key"].to_ascii_lowercase()
                ));
                "[REDACTED_SECRET_ASSIGNMENT]".to_string()
            })
            .into_owned();
        findings.extend(
            bearer
                .find_iter(&redacted)
                .map(|_| "authorization_value".into()),
        );
        redacted = bearer
            .replace_all(&redacted, "[REDACTED_AUTH]")
            .into_owned();
        findings.extend(
            provider_token
                .find_iter(&redacted)
                .map(|_| "provider_token".into()),
        );
        redacted = provider_token
            .replace_all(&redacted, "[REDACTED_TOKEN]")
            .into_owned();
        findings.extend(
            private_path
                .find_iter(&redacted)
                .map(|_| "private_absolute_path".into()),
        );
        redacted = private_path
            .replace_all(&redacted, "[REDACTED_PATH]")
            .into_owned();
        Ok(EvidencePreview { redacted, findings })
    }

    pub fn import_evidence(&self, input: EvidenceImport) -> Result<EvidenceEnvelope, SkillError> {
        if input.retention.delete_after_unix_ms <= input.created_at_unix_ms {
            return Err(SkillError::InvalidEvidenceRetention);
        }
        let skill_path = self.skill_source(&input.skill_id)?;
        read_ordinary_file(&skill_path).map_err(|_| SkillError::UnknownSkill)?;
        let preview = self.preview_evidence(&input.bytes)?;
        if preview.redacted.trim().is_empty() {
            return Err(SkillError::InvalidEvidence);
        }
        assert_no_embedded_secrets(&Value::String(preview.redacted.clone()))?;
        let content = self.artifacts.put(
            preview.redacted.as_bytes(),
            ArtifactSensitivity::LocalSensitive,
        )?;
        let report_bytes = canonical_json(&preview)?;
        let report = self
            .artifacts
            .put(&report_bytes, ArtifactSensitivity::LocalSensitive)?;
        let id_digest = digest_domain_json(
            "commonkit.skill-evidence-id.v1",
            &(
                &input.skill_id,
                &input.source_kind,
                &content.digest,
                input.created_at_unix_ms,
            ),
        )?;
        let id = StableId::parse(format!("evidence-{}", &digest_filename(&id_digest)[..20]))?;
        let envelope = EvidenceEnvelope {
            schema_version: SchemaVersion(1),
            id,
            skill_id: input.skill_id,
            source_kind: input.source_kind,
            content_digest: content.digest.clone(),
            content_bytes: content.bytes,
            sensitivity: ContentSensitivity::LocalSensitive,
            redaction_report_digest: report.digest.clone(),
            consent: input.consent,
            retention: input.retention,
            created_at_unix_ms: input.created_at_unix_ms,
        };
        let record = EvidenceRecord {
            envelope: envelope.clone(),
            content,
            report,
        };
        persist_immutable(&self.evidence_path(&envelope.id), &canonical_json(&record)?)?;
        Ok(envelope)
    }

    pub fn evidence(&self, id: &StableId) -> Result<EvidenceEnvelope, SkillError> {
        Ok(self.load_evidence(id)?.envelope)
    }

    pub fn evidence_preview(&self, id: &StableId) -> Result<String, SkillError> {
        let record = self.load_evidence(id)?;
        let bytes = self.artifacts.load(&record.content)?;
        String::from_utf8(bytes).map_err(|_| SkillError::InvalidEvidence)
    }

    /// Routes only explicitly imported, reviewed evidence to its selected
    /// skill. It never mines or classifies raw transcripts.
    pub fn opportunities(
        &self,
        minimum_evidence: u64,
    ) -> Result<Vec<SkillOpportunity>, SkillError> {
        if minimum_evidence == 0 {
            return Err(SkillError::InvalidOpportunityThreshold);
        }
        let mut counts = BTreeMap::<StableId, u64>::new();
        for entry in fs::read_dir(self.state.join("evidence"))? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let record: EvidenceRecord = read_json(&path)?;
            let count = counts.entry(record.envelope.skill_id).or_default();
            *count = count
                .checked_add(1)
                .ok_or(SkillError::InvalidOpportunityThreshold)?;
        }
        let mut opportunities = self
            .inventory()?
            .into_iter()
            .map(|skill| {
                let reviewed_evidence_count = counts.get(&skill.id).copied().unwrap_or(0);
                SkillOpportunity {
                    skill_id: skill.id,
                    reviewed_evidence_count,
                    eligible: reviewed_evidence_count >= minimum_evidence,
                }
            })
            .collect::<Vec<_>>();
        opportunities.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));
        Ok(opportunities)
    }

    pub fn candidate(&self, id: &StableId) -> Result<SkillCandidate, SkillError> {
        Ok(self.load_candidate(id)?.candidate)
    }

    pub fn candidates(&self) -> Result<Vec<SkillCandidate>, SkillError> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(self.state.join("candidates"))? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            candidates.push(read_json::<CandidateRecord>(&path)?.candidate);
        }
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(candidates)
    }

    pub fn plan_promotion(
        &self,
        candidate_id: &StableId,
        approval: Approval,
        repository_revision: GitRevision,
    ) -> Result<PromotionPlan, SkillError> {
        validate_approval(&approval)?;
        let record = self.load_candidate(candidate_id)?;
        if record.candidate.state != CandidateState::Approvable {
            return Err(SkillError::CandidateNotApprovable);
        }
        if repository_revision != record.repository_revision {
            return Err(SkillError::RepositoryRevisionMismatch);
        }
        if self.repository_revision()? != repository_revision
            || self.current_policy_digest()? != record.policy_digest
        {
            return Err(SkillError::PromotionContextStale);
        }
        let draft = PromotionPlanDraft {
            schema_version: SchemaVersion(1),
            candidate_id: candidate_id.clone(),
            candidate_digest: record.candidate.candidate_digest.clone(),
            parent_skill_digest: record.candidate.parent_skill_digest.clone(),
            source_path: record.source_path,
            repository_revision,
            policy_digest: record.policy_digest,
            suite_digest: record.suite_digest,
            approval,
        };
        let plan = PromotionPlan {
            schema_version: draft.schema_version,
            id: digest_domain_json("commonkit.skill-promotion-plan.v1", &draft)?,
            candidate_id: draft.candidate_id,
            candidate_digest: draft.candidate_digest,
            parent_skill_digest: draft.parent_skill_digest,
            source_path: draft.source_path,
            repository_revision: draft.repository_revision,
            policy_digest: draft.policy_digest,
            suite_digest: draft.suite_digest,
            approval: draft.approval,
        };
        persist_immutable(&self.plan_path(&plan.id), &canonical_json(&plan)?)?;
        self.append_event(candidate_id, "promotion_planned", &plan)?;
        Ok(plan)
    }

    pub fn apply_promotion(&self, plan: &PromotionPlan) -> Result<PromotionReceipt, SkillError> {
        validate_promotion_plan(plan)?;
        let stored: PromotionPlan = read_json(&self.plan_path(&plan.id))?;
        if stored != *plan {
            return Err(SkillError::PromotionPlanMismatch);
        }
        if self.repository_revision()? != plan.repository_revision
            || self.current_policy_digest()? != plan.policy_digest
        {
            return Err(SkillError::PromotionContextStale);
        }
        let record = self.load_candidate(&plan.candidate_id)?;
        if record.candidate.candidate_digest != plan.candidate_digest
            || record.candidate.parent_skill_digest != plan.parent_skill_digest
            || record.source_path != plan.source_path
        {
            return Err(SkillError::PromotionPlanMismatch);
        }
        let source = self.resolve_source(plan.source_path.as_str())?;
        let current = read_ordinary_file(&source)?;
        if digest_bytes(&current)? != plan.parent_skill_digest {
            return Err(SkillError::PromotionSourceChanged);
        }
        let candidate = self.artifacts.load(&record.candidate_content)?;
        if digest_bytes(&candidate)? != plan.candidate_digest {
            return Err(SkillError::PromotionPlanMismatch);
        }
        atomic_replace(&source, &candidate)?;
        if digest_bytes(&read_ordinary_file(&source)?)? != plan.candidate_digest {
            let parent = self.artifacts.load(&record.parent_content)?;
            atomic_replace(&source, &parent)?;
            return Err(SkillError::PromotionVerificationFailed);
        }
        let draft = PromotionReceiptDraft {
            schema_version: SchemaVersion(1),
            plan_id: plan.id.clone(),
            candidate_id: plan.candidate_id.clone(),
            source_path: plan.source_path.clone(),
            before: record.parent_content,
            after: record.candidate_content,
            state: PromotionState::Promoted,
        };
        let receipt = PromotionReceipt {
            schema_version: draft.schema_version,
            id: digest_domain_json("commonkit.skill-promotion-receipt.v1", &draft)?,
            plan_id: draft.plan_id,
            candidate_id: draft.candidate_id,
            source_path: draft.source_path,
            before: draft.before,
            after: draft.after,
            state: draft.state,
        };
        persist_immutable(&self.receipt_path(&receipt.id), &canonical_json(&receipt)?)?;
        self.append_event(&plan.candidate_id, "candidate_promoted", &receipt)?;
        Ok(receipt)
    }

    pub fn rollback_promotion(
        &self,
        receipt: &PromotionReceipt,
    ) -> Result<PromotionReceipt, SkillError> {
        let stored: PromotionReceipt = read_json(&self.receipt_path(&receipt.id))?;
        if stored != *receipt || receipt.state != PromotionState::Promoted {
            return Err(SkillError::PromotionReceiptMismatch);
        }
        let source = self.resolve_source(receipt.source_path.as_str())?;
        if digest_bytes(&read_ordinary_file(&source)?)? != receipt.after.digest {
            return Err(SkillError::RollbackSourceChanged);
        }
        let before = self.artifacts.load(&receipt.before)?;
        atomic_replace(&source, &before)?;
        if digest_bytes(&read_ordinary_file(&source)?)? != receipt.before.digest {
            return Err(SkillError::RollbackVerificationFailed);
        }
        let draft = PromotionReceiptDraft {
            schema_version: receipt.schema_version,
            plan_id: receipt.plan_id.clone(),
            candidate_id: receipt.candidate_id.clone(),
            source_path: receipt.source_path.clone(),
            before: receipt.before.clone(),
            after: receipt.after.clone(),
            state: PromotionState::RolledBack,
        };
        let rolled_back = PromotionReceipt {
            schema_version: draft.schema_version,
            id: digest_domain_json("commonkit.skill-promotion-receipt.v1", &draft)?,
            plan_id: draft.plan_id,
            candidate_id: draft.candidate_id,
            source_path: draft.source_path,
            before: draft.before,
            after: draft.after,
            state: draft.state,
        };
        persist_immutable(
            &self.receipt_path(&rolled_back.id),
            &canonical_json(&rolled_back)?,
        )?;
        self.append_event(&receipt.candidate_id, "promotion_rolled_back", &rolled_back)?;
        Ok(rolled_back)
    }

    fn load_candidate(&self, id: &StableId) -> Result<CandidateRecord, SkillError> {
        read_json(&self.candidate_path(id))
    }

    fn load_evidence(&self, id: &StableId) -> Result<EvidenceRecord, SkillError> {
        read_json(&self.evidence_path(id))
    }

    fn skill_source(&self, id: &StableId) -> Result<PathBuf, SkillError> {
        let path = self
            .repository
            .join(".agents/skills")
            .join(id.as_str())
            .join("SKILL.md");
        read_ordinary_file(&path).map_err(|_| SkillError::UnknownSkill)?;
        Ok(path)
    }

    fn repository_revision(&self) -> Result<GitRevision, SkillError> {
        let output = Command::new("git")
            .env_clear()
            .arg("-C")
            .arg(&self.repository)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !output.status.success() {
            return Err(SkillError::RepositoryRevisionUnavailable);
        }
        let revision = std::str::from_utf8(&output.stdout)
            .map_err(|_| SkillError::RepositoryRevisionUnavailable)?
            .trim();
        GitRevision::parse(revision).map_err(|_| SkillError::RepositoryRevisionUnavailable)
    }

    fn current_policy_digest(&self) -> Result<Sha256Digest, SkillError> {
        let policy = read_ordinary_file(
            &self
                .repository
                .join("policies/skill-optimization.policy.json"),
        )?;
        digest_bytes(&policy)
    }

    fn resolve_source(&self, relative: &str) -> Result<PathBuf, SkillError> {
        let candidate = self.repository.join(relative);
        let parent = candidate.parent().ok_or(SkillError::UnsafeSourcePath)?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(&self.repository) {
            return Err(SkillError::UnsafeSourcePath);
        }
        Ok(canonical_parent.join(candidate.file_name().ok_or(SkillError::UnsafeSourcePath)?))
    }

    fn candidate_path(&self, id: &StableId) -> PathBuf {
        self.state.join("candidates").join(format!("{id}.json"))
    }

    fn evidence_path(&self, id: &StableId) -> PathBuf {
        self.state.join("evidence").join(format!("{id}.json"))
    }

    fn plan_path(&self, id: &Sha256Digest) -> PathBuf {
        self.state
            .join("plans")
            .join(format!("{}.json", digest_filename(id)))
    }

    fn receipt_path(&self, id: &Sha256Digest) -> PathBuf {
        self.state
            .join("receipts")
            .join(format!("{}.json", digest_filename(id)))
    }

    fn append_event<T: Serialize>(
        &self,
        candidate: &StableId,
        kind: &str,
        data: &T,
    ) -> Result<(), SkillError> {
        let event = LifecycleEvent {
            sequence: next_event_sequence(&self.state.join("events"), candidate)?,
            candidate_id: candidate.clone(),
            kind: kind.into(),
            data: serde_json::to_value(data)?,
        };
        let path = self
            .state
            .join("events")
            .join(format!("{}-{:020}.json", candidate, event.sequence));
        persist_immutable(&path, &canonical_json(&event)?)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PromotionPlanDraft {
    schema_version: SchemaVersion,
    candidate_id: StableId,
    candidate_digest: Sha256Digest,
    parent_skill_digest: Sha256Digest,
    source_path: commonkit_contracts::PortableSourcePath,
    repository_revision: GitRevision,
    policy_digest: Sha256Digest,
    suite_digest: Sha256Digest,
    approval: Approval,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PromotionReceiptDraft {
    schema_version: SchemaVersion,
    plan_id: Sha256Digest,
    candidate_id: StableId,
    source_path: commonkit_contracts::PortableSourcePath,
    before: ContentReference,
    after: ContentReference,
    state: PromotionState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleEvent {
    sequence: u64,
    candidate_id: StableId,
    kind: String,
    data: Value,
}

pub fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, SkillError> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).map_err(Into::into)
}

/// Measures the complete installed provider environment. The self-describing
/// check file is excluded so it cannot authenticate itself.
pub fn measure_provider_installation(root: &Path) -> Result<Sha256Digest, SkillError> {
    digest_directory(
        root,
        Some("commonkit-provider-lock.json"),
        "commonkit.skillopt-installation.v1",
    )
}

/// Binds the executable, held-out corpus, and scorer implementation to the
/// suite's harness environment lock.
pub fn measure_harness_lock(
    kind: StableId,
    version: String,
    executable: &Path,
    corpus: &Path,
) -> Result<commonkit_contracts::HarnessLock, SkillError> {
    let executable_digest = digest_bytes(&read_ordinary_file(executable)?)?;
    let corpus_digest = digest_directory(corpus, None, "commonkit.skill-harness-corpus.v1")?;
    let scorer_digest = digest_bytes(&read_ordinary_file(&corpus.join("scorer"))?)?;
    let environment_digest = digest_domain_json(
        "commonkit.skill-harness-environment.v1",
        &(
            &kind,
            &version,
            executable_digest,
            corpus_digest,
            scorer_digest,
        ),
    )?;
    Ok(commonkit_contracts::HarnessLock {
        kind,
        version,
        environment_digest,
    })
}

fn digest_directory(
    root: &Path,
    excluded_root_file: Option<&str>,
    domain: &str,
) -> Result<Sha256Digest, SkillError> {
    let root = root.canonicalize()?;
    let mut pending = vec![root.clone()];
    let mut entries = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut children = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(SkillError::IntegrityMeasurementFailed);
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(|_| SkillError::IntegrityMeasurementFailed)?;
            if relative.components().count() == 1
                && excluded_root_file.is_some_and(|name| relative == Path::new(name))
            {
                continue;
            }
            let portable = relative
                .to_str()
                .ok_or(SkillError::IntegrityMeasurementFailed)?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if metadata.is_dir() {
                entries.push((portable, "directory", None));
                pending.push(path);
            } else if metadata.is_file() {
                entries.push((portable, "file", Some(digest_bytes(&fs::read(path)?)?)));
            } else {
                return Err(SkillError::IntegrityMeasurementFailed);
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(digest_domain_json(domain, &entries)?)
}

fn validate_candidate_content(bytes: &[u8]) -> Result<(), SkillError> {
    if bytes.is_empty() || bytes.len() > 256 * 1024 {
        return Err(SkillError::InvalidCandidateContent);
    }
    let content = std::str::from_utf8(bytes).map_err(|_| SkillError::InvalidCandidateContent)?;
    if !content.contains(|character: char| character.is_alphanumeric()) {
        return Err(SkillError::InvalidCandidateContent);
    }
    assert_no_embedded_secrets(&Value::String(content.into()))?;
    Ok(())
}

fn validate_approval(approval: &Approval) -> Result<(), SkillError> {
    if approval.approved_at_unix_ms == 0
        || approval.reason.trim().is_empty()
        || approval.reason.len() > 500
    {
        return Err(SkillError::InvalidApproval);
    }
    assert_no_embedded_secrets(&serde_json::to_value(approval)?)?;
    Ok(())
}

fn validate_promotion_plan(plan: &PromotionPlan) -> Result<(), SkillError> {
    validate_approval(&plan.approval)?;
    let draft = PromotionPlanDraft {
        schema_version: plan.schema_version,
        candidate_id: plan.candidate_id.clone(),
        candidate_digest: plan.candidate_digest.clone(),
        parent_skill_digest: plan.parent_skill_digest.clone(),
        source_path: plan.source_path.clone(),
        repository_revision: plan.repository_revision.clone(),
        policy_digest: plan.policy_digest.clone(),
        suite_digest: plan.suite_digest.clone(),
        approval: plan.approval.clone(),
    };
    if digest_domain_json("commonkit.skill-promotion-plan.v1", &draft)? != plan.id {
        return Err(SkillError::PromotionPlanMismatch);
    }
    Ok(())
}

fn validate_provider_lock(lock: &ProviderLock) -> Result<(), SkillError> {
    let required = ["reviewed-tasks", "staged-skill", "json-report"];
    if lock.id.as_str() != "skillopt"
        || lock.version != SKILLOPT_SUPPORTED_VERSION
        || lock.adapter_contract.as_str() != SKILLOPT_ADAPTER_CONTRACT
        || lock.source != ProviderSource::Pypi
        || !required.iter().all(|required| {
            lock.capabilities
                .iter()
                .any(|capability| capability.as_str() == *required)
        })
    {
        return Err(SkillError::UnsupportedSkillOptProvider);
    }
    Ok(())
}

fn provider_check_from_lock(
    lock: &ProviderLock,
    installed_content_digest: Sha256Digest,
) -> ProviderCheck {
    ProviderCheck {
        provider: lock.id.clone(),
        version: lock.version.clone(),
        adapter_contract: lock.adapter_contract.clone(),
        source: lock.source,
        package_digest: lock.package_digest.clone(),
        installed_content_digest,
        capabilities: lock.capabilities.clone(),
        compatible: true,
    }
}

fn validate_upgrade_plan(plan: &ProviderUpgradePlan) -> Result<(), SkillError> {
    validate_provider_lock(&plan.provider_lock)?;
    let fixture_digest = digest_domain_json(
        "commonkit.skillopt-provider-fixtures.v1",
        &(
            digest_bytes(&plan.fixture_skill)?,
            digest_bytes(&plan.reviewed_tasks)?,
        ),
    )?;
    let expected = digest_domain_json(
        "commonkit.skillopt-provider-upgrade-plan.v1",
        &(&plan.provider_lock, &fixture_digest),
    )?;
    if fixture_digest != plan.fixture_digest || expected != plan.id {
        return Err(SkillError::UpgradePlanMismatch);
    }
    Ok(())
}

fn validate_upgrade_report(report: &ProviderUpgradeReport) -> Result<(), SkillError> {
    if !report.compatible
        || !report.source_unchanged
        || report.candidate_basis_points <= report.baseline_basis_points
    {
        return Err(SkillError::UpgradeReportRejected);
    }
    let draft = ProviderUpgradeReportDraft {
        plan_id: report.plan_id.clone(),
        provider_lock: report.provider_lock.clone(),
        environment_root: report.environment_root.clone(),
        requirements_digest: report.requirements_digest.clone(),
        baseline_basis_points: report.baseline_basis_points,
        candidate_basis_points: report.candidate_basis_points,
        source_unchanged: report.source_unchanged,
        compatible: report.compatible,
    };
    if digest_domain_json("commonkit.skillopt-provider-upgrade-report.v1", &draft)? != report.id {
        return Err(SkillError::UpgradeReportRejected);
    }
    Ok(())
}

fn run_uv<const N: usize>(
    uv: &Path,
    working_directory: &Path,
    args: [OsString; N],
) -> Result<(), SkillError> {
    let stdout_path = working_directory.join(format!(
        "uv-{}.stdout",
        TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let stderr_path = working_directory.join(format!(
        "uv-{}.stderr",
        TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut child = Command::new(uv)
        .env_clear()
        .env("HOME", working_directory)
        .current_dir(working_directory)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(private_output_file(&stdout_path)?))
        .stderr(Stdio::from(private_output_file(&stderr_path)?))
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(SkillError::ProviderInstallFailed);
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SkillError::ProviderTimedOut);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn provider_fixture_suite() -> Result<SkillEvaluationSuite, SkillError> {
    let id = |value: &str| StableId::parse(value);
    let digest = |value: &[u8]| digest_bytes(value);
    Ok(SkillEvaluationSuite {
        schema_version: SchemaVersion(1),
        id: id("provider-contract")?,
        skill_id: id("contract-fixture")?,
        train: commonkit_contracts::EvaluationCaseManifest {
            content_digest: digest(b"train")?,
            case_ids: BTreeSet::from([id("wrap-train")?]),
        },
        validation: commonkit_contracts::EvaluationCaseManifest {
            content_digest: digest(b"validation")?,
            case_ids: BTreeSet::from([id("wrap-validation")?]),
        },
        held_out: commonkit_contracts::EvaluationCaseManifest {
            content_digest: digest(b"held-out")?,
            case_ids: BTreeSet::from([id("wrap-held-out")?]),
        },
        rubric_digest: digest(b"strict mock fixture")?,
        harness: commonkit_contracts::HarnessLock {
            kind: id("skillopt-sleep")?,
            version: SKILLOPT_SUPPORTED_VERSION.into(),
            environment_digest: digest(b"skillopt-sleep-v1-mock")?,
        },
        metric: commonkit_contracts::EvaluationMetric {
            id: id("accuracy")?,
            minimum_improvement_basis_points: 1,
            maximum_held_out_regression_basis_points: 0,
            required_case_ids: BTreeSet::from([id("wrap-held-out")?]),
        },
    })
}

pub fn measured_provider_fixture_suite(
    harness_executable: &Path,
    harness_corpus: &Path,
) -> Result<SkillEvaluationSuite, SkillError> {
    let mut suite = provider_fixture_suite()?;
    suite.harness = measure_harness_lock(
        suite.harness.kind.clone(),
        suite.harness.version.clone(),
        harness_executable,
        harness_corpus,
    )?;
    Ok(suite)
}

fn provider_fixture_manifest(
    provider: ProviderLock,
    source_digest: Sha256Digest,
    suite: &SkillEvaluationSuite,
) -> Result<SkillOptimizationManifest, SkillError> {
    let id = |value: &str| StableId::parse(value);
    Ok(SkillOptimizationManifest {
        schema_version: SchemaVersion(1),
        id: id("provider-contract-run")?,
        skill: commonkit_contracts::SkillDescriptor {
            id: id("contract-fixture")?,
            source_path: commonkit_contracts::PortableSourcePath::parse(
                "skills/contract-fixture/SKILL.md",
            )?,
            source_digest,
            package: None,
            targets: BTreeSet::from([id("codex")?]),
            lifecycle: commonkit_contracts::SkillLifecycle::Experimental,
        },
        suite_digest: suite.digest()?,
        evidence_digests: Vec::new(),
        provider,
        optimizer: commonkit_contracts::ModelLock {
            provider: id("skillopt")?,
            model: "mock".into(),
        },
        target: commonkit_contracts::ModelLock {
            provider: id("skillopt")?,
            model: "mock".into(),
        },
        limits: commonkit_contracts::OptimizationLimits {
            maximum_cases: 3,
            maximum_edits: 2,
            timeout_seconds: 60,
            maximum_cost_micros: 1,
        },
        policy_digest: digest_bytes(b"provider-contract-policy")?,
        repository_revision: GitRevision::parse("0".repeat(40))?,
    })
}

fn validate_provider_environment(environment: &BTreeMap<String, String>) -> Result<(), SkillError> {
    let allowed = [
        "ANTHROPIC_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_AUTH_MODE",
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_API_VERSION",
        "OPENAI_API_KEY",
    ];
    if environment.iter().any(|(name, value)| {
        !allowed.contains(&name.as_str()) || value.is_empty() || value.contains(['\0', '\n', '\r'])
    }) {
        return Err(SkillError::InvalidProviderEnvironmentVariable);
    }
    Ok(())
}

fn direct_requirement_has_hash(
    requirements: &str,
    package: &str,
    version: &str,
    expected_hash: &str,
) -> bool {
    let requirement = format!("{package}=={version}");
    let hash = format!("sha256:{expected_hash}");
    let mut lines = requirements.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(&requirement) {
            continue;
        }
        if trimmed
            .get(requirement.len()..)
            .is_some_and(|suffix| !suffix.is_empty() && !suffix.starts_with(char::is_whitespace))
        {
            continue;
        }
        if line.contains(&hash) {
            return true;
        }
        while line.trim_end().ends_with('\\') {
            let Some(continuation) = lines.next() else {
                break;
            };
            if continuation.contains(&hash) {
                return true;
            }
            if !continuation.trim_end().ends_with('\\') {
                break;
            }
        }
        return false;
    }
    false
}

#[cfg(test)]
mod provider_requirement_tests {
    use super::direct_requirement_has_hash;

    #[test]
    fn hash_must_belong_to_the_direct_skillopt_requirement() {
        let digest = "818db802507c6f82553fd24c75aa70c953ab0a712647f60e68e4595052c4b150";
        let unrelated = format!("skillopt==0.2.0\ndependency==1.0 --hash=sha256:{digest}\n");
        assert!(!direct_requirement_has_hash(
            &unrelated, "skillopt", "0.2.0", digest
        ));
        let direct = format!("skillopt==0.2.0 --hash=sha256:{digest}\n");
        assert!(direct_requirement_has_hash(
            &direct, "skillopt", "0.2.0", digest
        ));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod provider_sandbox_tests {
    use super::{create_private_staging, isolated_command};
    use std::path::Path;

    #[test]
    fn narrow_profile_can_start_the_system_shell_from_staging() {
        let staging = create_private_staging().expect("staging");
        let mut command =
            isolated_command(Path::new("/bin/sh"), &staging, &[]).expect("sandbox command");
        let status = command
            .arg("-c")
            .arg("true")
            .status()
            .expect("execute sandboxed shell");
        let _ = std::fs::remove_dir_all(staging);
        assert!(status.success());
    }
}

fn validate_reviewed_tasks_shape(path: &Path) -> Result<Value, SkillError> {
    let bytes = read_bounded(path, 8 * 1024 * 1024)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    if value.get("format").and_then(Value::as_str) != Some("skillopt_sleep.tasks.v1")
        || value.get("reviewed").and_then(Value::as_bool) != Some(true)
        || value
            .get("tasks")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err(SkillError::UnreviewedTasksFile);
    }
    assert_no_embedded_secrets(&value)?;
    Ok(value)
}

fn validate_reviewed_tasks(
    path: &Path,
    manifest: &SkillOptimizationManifest,
    suite: &SkillEvaluationSuite,
) -> Result<(), SkillError> {
    let value = validate_reviewed_tasks_shape(path)?;
    let binding: ReviewedTasksBinding = serde_json::from_value(
        value
            .get("commonkit")
            .cloned()
            .ok_or(SkillError::TasksBindingMismatch)?,
    )
    .map_err(|_| SkillError::TasksBindingMismatch)?;
    if binding.skill_id != manifest.skill.id
        || binding.campaign_id != suite.id
        || binding.suite_digest != suite.digest()?
        || binding.train_digest != suite.train.content_digest
        || binding.validation_digest != suite.validation.content_digest
    {
        return Err(SkillError::TasksBindingMismatch);
    }
    let task_ids = value
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or(SkillError::TasksBindingMismatch)?
        .iter()
        .map(|task| {
            task.get("id")
                .and_then(Value::as_str)
                .ok_or(SkillError::TasksBindingMismatch)
                .and_then(|id| StableId::parse(id).map_err(|_| SkillError::TasksBindingMismatch))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let visible = suite
        .train
        .case_ids
        .union(&suite.validation.case_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    if task_ids != visible || !task_ids.is_disjoint(&suite.held_out.case_ids) {
        return Err(SkillError::TasksBindingMismatch);
    }
    Ok(())
}

fn create_private_staging() -> Result<PathBuf, SkillError> {
    create_private_staging_named("commonkit-skillopt")
}

fn create_private_staging_named(prefix: &str) -> Result<PathBuf, SkillError> {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path)?;
    set_private_directory(&path)?;
    Ok(path.canonicalize()?)
}

struct PrivateStaging(PathBuf);

impl PrivateStaging {
    fn new(prefix: &str) -> Result<Self, SkillError> {
        Ok(Self(create_private_staging_named(prefix)?))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateStaging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private_output_file(path: &Path) -> Result<fs::File, SkillError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, SkillError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > maximum {
        return Err(SkillError::ProviderOutputIncompatible);
    }
    Ok(fs::read(path)?)
}

fn file_exceeds(path: &Path, maximum: u64) -> Result<bool, SkillError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SkillError::ProviderOutputIncompatible);
    }
    Ok(metadata.len() > maximum)
}

fn run_bounded_child(
    command: &mut Command,
    stdout: &Path,
    stderr: &Path,
    timeout_seconds: u32,
) -> Result<(), SkillError> {
    let mut child = command.spawn()?;
    let process_group = child.id();
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds.into());
    loop {
        if let Some(status) = child.try_wait()? {
            let result = if status.success() {
                Ok(())
            } else {
                Err(SkillError::HarnessExecutionFailed)
            };
            terminate_process_group(&mut child, process_group);
            return result;
        }
        if file_exceeds(stdout, 2 * 1024 * 1024)? || file_exceeds(stderr, 2 * 1024 * 1024)? {
            terminate_process_group(&mut child, process_group);
            return Err(SkillError::HarnessOutputIncompatible);
        }
        if Instant::now() >= deadline {
            terminate_process_group(&mut child, process_group);
            return Err(SkillError::ProviderTimedOut);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut std::process::Child, process_group: u32) {
    // The child is always placed in a fresh process group by isolated_command.
    // A negative kill target reaches descendants even after the leader exits.
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{process_group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut std::process::Child, _process_group: u32) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "macos")]
fn isolated_command(
    executable: &Path,
    staging: &Path,
    extra_read_roots: &[&Path],
) -> Result<Command, SkillError> {
    fn quoted(path: &Path) -> Result<String, SkillError> {
        let value = path
            .to_str()
            .ok_or(SkillError::ProviderIsolationUnavailable)?;
        if value.contains(['\n', '\r', '"', '\\']) {
            return Err(SkillError::ProviderIsolationUnavailable);
        }
        Ok(format!("\"{value}\""))
    }
    let mut reads = vec![
        "/System".to_owned(),
        "/usr".to_owned(),
        "/bin".to_owned(),
        "/private/etc".to_owned(),
        "/dev".to_owned(),
        executable
            .parent()
            .ok_or(SkillError::ProviderIsolationUnavailable)?
            .to_string_lossy()
            .into_owned(),
        staging.to_string_lossy().into_owned(),
    ];
    reads.extend(
        extra_read_roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned()),
    );
    let mut literal_reads = vec!["/".to_owned(), "/private/var/select/sh".to_owned()];
    for root in std::iter::once(staging)
        .chain(std::iter::once(executable))
        .chain(extra_read_roots.iter().copied())
    {
        literal_reads.extend(
            root.ancestors()
                .skip(1)
                .filter(|path| path != &Path::new("/"))
                .map(|path| path.to_string_lossy().into_owned()),
        );
    }
    literal_reads.sort();
    literal_reads.dedup();
    let read_rules = reads
        .iter()
        .map(|path| quoted(Path::new(path)).map(|path| format!("(subpath {path})")))
        .chain(
            literal_reads
                .iter()
                .map(|path| quoted(Path::new(path)).map(|path| format!("(literal {path})"))),
        )
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let profile = format!(
        "(version 1) (deny default) (allow process*) (allow sysctl-read) \
         (allow file-read* {read_rules}) (allow file-write* (subpath {})) \
         (deny network*)",
        quoted(staging)?
    );
    let sandbox = Path::new("/usr/bin/sandbox-exec");
    if !sandbox.is_file() {
        return Err(SkillError::ProviderIsolationUnavailable);
    }
    let mut command = Command::new(sandbox);
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command
        .arg("-p")
        .arg(profile)
        .arg(executable)
        .current_dir(staging);
    Ok(command)
}

#[cfg(not(target_os = "macos"))]
fn isolated_command(
    _executable: &Path,
    _staging: &Path,
    _extra_read_roots: &[&Path],
) -> Result<Command, SkillError> {
    Err(SkillError::ProviderIsolationUnavailable)
}

fn score_basis_points(score: f64) -> Result<u32, SkillError> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(SkillError::ProviderOutputIncompatible);
    }
    Ok((score * 10_000.0).round() as u32)
}

fn render_patch(before: &[u8], after: &[u8], path: &str) -> Vec<u8> {
    format!(
        "--- a/{path}\n+++ b/{path}\n@@ replacement @@\n-{}\n+{}\n",
        String::from_utf8_lossy(before).replace('\n', "\\n"),
        String::from_utf8_lossy(after).replace('\n', "\\n")
    )
    .into_bytes()
}

fn candidate_id(manifest: &Sha256Digest, candidate: &Sha256Digest) -> Result<StableId, SkillError> {
    let digest = digest_domain_json("commonkit.skill-candidate-id.v1", &(manifest, candidate))?;
    StableId::parse(format!("candidate-{}", &digest_filename(&digest)[..20])).map_err(Into::into)
}

fn digest_filename(digest: &Sha256Digest) -> &str {
    digest.as_str().trim_start_matches("sha256:")
}

fn read_ordinary_file(path: &Path) -> Result<Vec<u8>, SkillError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SkillError::UnsafeSourceType);
    }
    Ok(fs::read(path)?)
}

fn persist_immutable(path: &Path, bytes: &[u8]) -> Result<(), SkillError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
            sync_parent(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(path)? == bytes {
                Ok(())
            } else {
                Err(SkillError::ImmutableConflict)
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, SkillError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SkillError::UnsafeStateType);
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn validate_marker_timestamp(value: &str) -> Result<(), SkillError> {
    let bytes = value.as_bytes();
    let separators = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    if bytes.len() != 20
        || separators
            .iter()
            .any(|(index, expected)| bytes[*index] != *expected)
        || bytes.iter().enumerate().any(|(index, byte)| {
            !separators.iter().any(|(separator, _)| *separator == index) && !byte.is_ascii_digit()
        })
    {
        return Err(SkillError::InvalidActivityMarker);
    }
    Ok(())
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), SkillError> {
    let parent = path.parent().ok_or(SkillError::UnsafeSourcePath)?;
    let temporary = parent.join(format!(
        ".commonkit-skill-{}-{}.tmp",
        std::process::id(),
        TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = fs::metadata(path)?.permissions().mode();
        options.mode(mode);
    }
    let result = (|| -> Result<(), SkillError> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SkillError> {
    let parent = path.parent().ok_or(SkillError::UnsafeSourcePath)?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".commonkit-provider-lock-{}.tmp",
        TEMP_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<(), SkillError> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn next_event_sequence(directory: &Path, candidate: &StableId) -> Result<u64, SkillError> {
    let prefix = format!("{candidate}-");
    let mut next = 0;
    for entry in fs::read_dir(directory)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(sequence) = name
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(".json"))
            .and_then(|value| value.parse::<u64>().ok())
        {
            next = next.max(sequence + 1);
        }
    }
    Ok(next)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("Claude SessionEnd activity marker log is invalid")]
    InvalidActivityMarker,
    #[error("skill opportunity threshold is invalid")]
    InvalidOpportunityThreshold,
    #[error("skill schedule is invalid")]
    InvalidSchedule,
    #[error("evaluation suite does not match the optimization manifest")]
    SuiteMismatch,
    #[error("skill source changed before optimization")]
    SourceChangedBeforeOptimization,
    #[error("candidate content is invalid")]
    InvalidCandidateContent,
    #[error("evidence content is invalid")]
    InvalidEvidence,
    #[error("evidence retention must end after creation")]
    InvalidEvidenceRetention,
    #[error("evidence references an unknown skill")]
    UnknownSkill,
    #[error("unsupported SkillOpt provider version")]
    UnsupportedSkillOptProvider,
    #[error("SkillOpt provider configuration is invalid")]
    InvalidProviderConfiguration,
    #[error("SkillOpt provider environment is invalid")]
    InvalidProviderEnvironment,
    #[error("SkillOpt provider environment variable is not allowed")]
    InvalidProviderEnvironmentVariable,
    #[error("SkillOpt provider OS isolation is unavailable")]
    ProviderIsolationUnavailable,
    #[error("SkillOpt provider descendant accessed held-out harness state")]
    ProviderIsolationBreached,
    #[error("SkillOpt provider version probe failed")]
    ProviderVersionProbeFailed,
    #[error("optimization manifest does not match the configured provider lock")]
    ProviderLockMismatch,
    #[error("SkillOpt reviewed task file is invalid or not explicitly reviewed")]
    UnreviewedTasksFile,
    #[error("SkillOpt reviewed tasks are not bound to the selected skill and evaluation suite")]
    TasksBindingMismatch,
    #[error("SkillOpt provider timed out")]
    ProviderTimedOut,
    #[error("SkillOpt provider execution failed")]
    ProviderExecutionFailed,
    #[error("SkillOpt provider mutated its staged input skill")]
    ProviderMutatedInput,
    #[error("SkillOpt provider output is incompatible with the adapter contract")]
    ProviderOutputIncompatible,
    #[error("SkillOpt provider rejected the candidate")]
    ProviderCandidateRejected,
    #[error("skill evaluation harness is invalid")]
    InvalidHarness,
    #[error("skill evaluation harness failed")]
    HarnessExecutionFailed,
    #[error("skill evaluation harness output is incompatible")]
    HarnessOutputIncompatible,
    #[error("skill evaluation harness, corpus, or scorer does not match its lock")]
    HarnessIntegrityMismatch,
    #[error("provider or harness content could not be measured safely")]
    IntegrityMeasurementFailed,
    #[error("SkillOpt provider output escaped its staging directory")]
    ProviderOutputEscapedStaging,
    #[error("SkillOpt provider manager is invalid")]
    InvalidProviderManager,
    #[error("SkillOpt provider upgrade already exists")]
    UpgradeAlreadyExists,
    #[error("SkillOpt provider requirements do not contain the locked package hash")]
    ProviderRequirementsMismatch,
    #[error("SkillOpt provider installation failed")]
    ProviderInstallFailed,
    #[error("SkillOpt provider environment is already installed")]
    ProviderEnvironmentAlreadyInstalled,
    #[error("SkillOpt provider upgrade plan does not match its inputs")]
    UpgradePlanMismatch,
    #[error("SkillOpt provider upgrade report is not approvable")]
    UpgradeReportRejected,
    #[error("candidate is not approvable")]
    CandidateNotApprovable,
    #[error("approval metadata is invalid")]
    InvalidApproval,
    #[error("repository revision does not match the candidate")]
    RepositoryRevisionMismatch,
    #[error("Git repository revision is unavailable")]
    RepositoryRevisionUnavailable,
    #[error("optimization repository revision or policy is stale")]
    OptimizationContextStale,
    #[error("promotion repository revision or policy changed after approval")]
    PromotionContextStale,
    #[error("promotion plan does not match its durable inputs")]
    PromotionPlanMismatch,
    #[error("promotion source changed after planning")]
    PromotionSourceChanged,
    #[error("promotion verification failed")]
    PromotionVerificationFailed,
    #[error("promotion receipt does not match durable state")]
    PromotionReceiptMismatch,
    #[error("promoted source changed before rollback")]
    RollbackSourceChanged,
    #[error("rollback verification failed")]
    RollbackVerificationFailed,
    #[error("source path escapes the repository")]
    UnsafeSourcePath,
    #[error("skill source is not an ordinary file")]
    UnsafeSourceType,
    #[error("lifecycle state is not an ordinary file")]
    UnsafeStateType,
    #[error("immutable lifecycle object already exists with different content")]
    ImmutableConflict,
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

//! Stable, side-effect-free CommonKit wire contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::{fmt, sync::OnceLock};

use regex::Regex;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CONTRACT_VERSION: &str = "1.0";
pub const SCHEMA_VERSION: u32 = 1;

pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ContractError> {
    serde_jcs::to_vec(value).map_err(|_| ContractError::Canonicalization)
}

pub fn digest_json<T: Serialize + ?Sized>(value: &T) -> Result<Sha256Digest, ContractError> {
    let bytes = canonical_json(value)?;
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}"))
}

pub fn digest_domain_json<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<Sha256Digest, ContractError> {
    let canonical = canonical_json(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(canonical);
    let digest = hasher.finalize();
    Sha256Digest::parse(format!("sha256:{digest:x}"))
}

pub fn assert_no_embedded_secrets(value: &Value) -> Result<(), ContractError> {
    fn visit(value: &Value, location: &str) -> Result<(), ContractError> {
        match value {
            Value::String(value) => assert_safe_string(value, location),
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    visit(value, &format!("{location}/{index}"))?;
                }
                Ok(())
            }
            Value::Object(values) => {
                for (key, value) in values {
                    let child_location = format!("{location}/{}", escape_json_pointer(key));
                    if is_secret_key(key)
                        && value_can_contain_secret(value)
                        && !is_empty_or_false(value)
                    {
                        let safe_reference = value.as_str().is_some_and(is_reference);
                        if !safe_reference {
                            return Err(ContractError::EmbeddedSecret(child_location));
                        }
                    }
                    visit(value, &child_location)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    visit(value, "")
}

fn assert_safe_string(value: &str, location: &str) -> Result<(), ContractError> {
    static AUTH: OnceLock<Regex> = OnceLock::new();
    static URL_USERINFO: OnceLock<Regex> = OnceLock::new();
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    let auth = AUTH.get_or_init(|| {
        Regex::new(r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9+/_.=-]{8,}").expect("auth regex")
    });
    let url_userinfo = URL_USERINFO.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@").expect("URL regex")
    });
    let assignment = ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)["']?(?P<key>[A-Za-z_][A-Za-z0-9_.-]*)["']?\s*=\s*["']?(?P<value>[^\s,"'}#]+)"#,
        )
        .expect("assignment regex")
    });

    if auth.is_match(value) || url_userinfo.is_match(value) {
        return Err(ContractError::EmbeddedSecret(location.into()));
    }
    for captures in assignment.captures_iter(value) {
        let key = &captures["key"];
        let assigned = &captures["value"];
        if is_secret_key(key) && !is_reference(assigned) {
            return Err(ContractError::EmbeddedSecret(location.into()));
        }
    }
    Ok(())
}

fn is_secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    [
        "apikey",
        "token",
        "secret",
        "password",
        "passwd",
        "authorization",
        "credential",
        "privatekey",
    ]
    .iter()
    .any(|marker| normalized == *marker || normalized.ends_with(marker))
}

fn is_reference(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with('$')
        || ["env:", "secret:", "bws:", "keychain:", "vault:"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

fn is_empty_or_false(value: &Value) -> bool {
    matches!(value, Value::Null | Value::Bool(false)) || value.as_str() == Some("")
}

fn value_can_contain_secret(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Array(_) | Value::Object(_))
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct StableId(String);

impl StableId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        let valid_length = !value.is_empty() && value.len() <= 63;
        let mut characters = value.chars();
        let valid_start = characters
            .next()
            .is_some_and(|character| character.is_ascii_lowercase());
        let valid_rest = characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        });
        if valid_length && valid_start && valid_rest {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidStableId(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StableId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        let digest = value.strip_prefix("sha256:");
        if digest.is_some_and(|digest| {
            digest.len() == 64
                && digest.chars().all(|character| {
                    character.is_ascii_hexdigit() && !character.is_ascii_uppercase()
                })
        }) {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidSha256Digest(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct PortableSourcePath(String);

impl PortableSourcePath {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        let segments: Vec<_> = value.split('/').collect();
        let valid = !value.is_empty()
            && !value.starts_with('/')
            && !value.contains('\\')
            && !value.contains('\0')
            && !value.contains(':')
            && segments
                .iter()
                .all(|segment| !segment.is_empty() && !matches!(*segment, "." | ".."));
        if valid {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidPortableSourcePath(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PortableSourcePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct GitRevision(String);

impl GitRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        let valid = matches!(value.len(), 40 | 64)
            && value
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase());
        if valid {
            Ok(Self(value))
        } else {
            Err(ContractError::InvalidGitRevision(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GitRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SkillLifecycle {
    Active,
    Library,
    Experimental,
    Retired,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ContentSensitivity {
    Portable,
    LocalSensitive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillDescriptor {
    pub id: StableId,
    pub source_path: PortableSourcePath,
    pub source_digest: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<StableId>,
    pub targets: BTreeSet<StableId>,
    pub lifecycle: SkillLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationCaseManifest {
    pub content_digest: Sha256Digest,
    pub case_ids: BTreeSet<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessLock {
    pub kind: StableId,
    pub version: String,
    pub environment_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationMetric {
    pub id: StableId,
    pub minimum_improvement_basis_points: u32,
    pub maximum_held_out_regression_basis_points: u32,
    pub required_case_ids: BTreeSet<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillEvaluationSuite {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub skill_id: StableId,
    pub train: EvaluationCaseManifest,
    pub validation: EvaluationCaseManifest,
    pub held_out: EvaluationCaseManifest,
    pub rubric_digest: Sha256Digest,
    pub harness: HarnessLock,
    pub metric: EvaluationMetric,
}

impl SkillEvaluationSuite {
    pub fn validate(&self) -> Result<(), ContractError> {
        let mut seen = BTreeSet::new();
        for manifest in [&self.train, &self.validation, &self.held_out] {
            for case_id in &manifest.case_ids {
                if !seen.insert(case_id.clone()) {
                    return Err(ContractError::EvaluationCaseOverlap(case_id.to_string()));
                }
            }
        }
        if self.train.case_ids.is_empty()
            || self.validation.case_ids.is_empty()
            || self.held_out.case_ids.is_empty()
        {
            return Err(ContractError::EmptyEvaluationSplit);
        }
        if !self
            .metric
            .required_case_ids
            .is_subset(&self.held_out.case_ids)
        {
            return Err(ContractError::RequiredCaseOutsideHeldOut);
        }
        validate_lock_text(&self.harness.version, "harness version")
    }

    pub fn digest(&self) -> Result<Sha256Digest, ContractError> {
        self.validate()?;
        digest_domain_json("commonkit.skill-evaluation-suite.v1", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderLock {
    pub id: StableId,
    pub version: String,
    pub adapter_contract: StableId,
    pub source: ProviderSource,
    pub package_digest: Sha256Digest,
    pub capabilities: BTreeSet<StableId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<GitRevision>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    Pypi,
    Container,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelLock {
    pub provider: StableId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptimizationLimits {
    pub maximum_cases: u32,
    pub maximum_edits: u32,
    pub timeout_seconds: u32,
    pub maximum_cost_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillOptimizationManifest {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub skill: SkillDescriptor,
    pub suite_digest: Sha256Digest,
    pub evidence_digests: Vec<Sha256Digest>,
    pub provider: ProviderLock,
    pub optimizer: ModelLock,
    pub target: ModelLock,
    pub limits: OptimizationLimits,
    pub policy_digest: Sha256Digest,
    pub repository_revision: GitRevision,
}

impl SkillOptimizationManifest {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.skill.targets.is_empty() {
            return Err(ContractError::MissingSkillTarget);
        }
        validate_lock_text(&self.provider.version, "provider version")?;
        if self.provider.id.as_str() == "skillopt"
            && (self.provider.version != "0.2.0"
                || self.provider.adapter_contract.as_str() != "skillopt-sleep-v1"
                || self.provider.source != ProviderSource::Pypi
                || !["reviewed-tasks", "staged-skill", "json-report"]
                    .iter()
                    .all(|capability| {
                        self.provider
                            .capabilities
                            .iter()
                            .any(|present| present.as_str() == *capability)
                    }))
        {
            return Err(ContractError::UnsupportedProviderVersion);
        }
        validate_lock_text(&self.optimizer.model, "optimizer model")?;
        validate_lock_text(&self.target.model, "target model")?;
        if self.limits.maximum_cases == 0
            || self.limits.maximum_edits == 0
            || self.limits.timeout_seconds == 0
            || self.limits.maximum_cost_micros == 0
        {
            return Err(ContractError::InvalidOptimizationLimits);
        }
        assert_no_embedded_secrets(
            &serde_json::to_value(self).map_err(|_| ContractError::Canonicalization)?,
        )
    }

    pub fn digest(&self) -> Result<Sha256Digest, ContractError> {
        self.validate()?;
        digest_domain_json("commonkit.skill-optimization-manifest.v1", self)
    }
}

fn validate_lock_text(value: &str, field: &'static str) -> Result<(), ContractError> {
    if value.trim().is_empty() || value.len() > 200 || value.contains(['\0', '\n', '\r']) {
        Err(ContractError::InvalidLockText(field))
    } else {
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationOutcome {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationReceipt {
    pub schema_version: SchemaVersion,
    pub baseline_basis_points: u32,
    pub candidate_basis_points: u32,
    pub held_out_baseline_basis_points: u32,
    pub held_out_candidate_basis_points: u32,
    pub required_cases: BTreeMap<StableId, EvaluationOutcome>,
    pub cost_micros: u64,
    pub harness: HarnessLock,
    pub scorer_digest: Sha256Digest,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Staged,
    Rejected,
    Approvable,
    Approved,
    Promoted,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCandidate {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub manifest_digest: Sha256Digest,
    pub parent_skill_digest: Sha256Digest,
    pub provider: ProviderLock,
    pub optimizer: ModelLock,
    pub target: ModelLock,
    pub configuration_digest: Sha256Digest,
    pub input_digests: Vec<Sha256Digest>,
    pub candidate_digest: Sha256Digest,
    pub candidate_bytes: u64,
    pub candidate_sensitivity: ContentSensitivity,
    pub patch_digest: Sha256Digest,
    pub evaluation: EvaluationReceipt,
    pub optimization_history_digest: Sha256Digest,
    pub policy_passed: bool,
    pub state: CandidateState,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSourceKind {
    ManualFailure,
    ClaudeSession,
    CodexSession,
    EvaluationCase,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceConsent {
    LocalOnly,
    ApprovedForProvider,
    ApprovedPortable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceRetention {
    pub delete_after_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceEnvelope {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub skill_id: StableId,
    pub source_kind: EvidenceSourceKind,
    pub content_digest: Sha256Digest,
    pub content_bytes: u64,
    pub sensitivity: ContentSensitivity,
    pub redaction_report_digest: Sha256Digest,
    pub consent: EvidenceConsent,
    pub retention: EvidenceRetention,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLifecycleSchema {
    pub descriptor: SkillDescriptor,
    pub suite: SkillEvaluationSuite,
    pub manifest: SkillOptimizationManifest,
    pub candidate: SkillCandidate,
    pub evidence: EvidenceEnvelope,
}

impl SkillCandidate {
    pub fn validate_against(
        &self,
        manifest: &SkillOptimizationManifest,
        suite: &SkillEvaluationSuite,
    ) -> Result<(), ContractError> {
        manifest.validate()?;
        suite.validate()?;
        if self.manifest_digest != manifest.digest()? {
            return Err(ContractError::CandidateManifestMismatch);
        }
        if self.parent_skill_digest != manifest.skill.source_digest {
            return Err(ContractError::CandidateParentMismatch);
        }
        if self.provider != manifest.provider
            || self.optimizer != manifest.optimizer
            || self.target != manifest.target
            || self.configuration_digest
                != digest_domain_json("commonkit.skillopt-configuration.v1", &manifest.limits)?
        {
            return Err(ContractError::CandidateProviderMismatch);
        }
        let mut expected_inputs = vec![
            manifest.skill.source_digest.clone(),
            manifest.suite_digest.clone(),
            manifest.policy_digest.clone(),
        ];
        expected_inputs.extend(manifest.evidence_digests.clone());
        if self.input_digests != expected_inputs {
            return Err(ContractError::CandidateInputMismatch);
        }
        if self.evaluation.harness != suite.harness {
            return Err(ContractError::CandidateHarnessMismatch);
        }
        if self.candidate_bytes == 0 || !self.policy_passed {
            return Err(ContractError::CandidatePolicyFailed);
        }
        let improvement = self
            .evaluation
            .candidate_basis_points
            .saturating_sub(self.evaluation.baseline_basis_points);
        if improvement < suite.metric.minimum_improvement_basis_points {
            return Err(ContractError::CandidateInsufficientImprovement);
        }
        let regression = self
            .evaluation
            .held_out_baseline_basis_points
            .saturating_sub(self.evaluation.held_out_candidate_basis_points);
        if regression > suite.metric.maximum_held_out_regression_basis_points {
            return Err(ContractError::CandidateHeldOutRegression);
        }
        if suite.metric.required_case_ids.iter().any(|case_id| {
            self.evaluation.required_cases.get(case_id) != Some(&EvaluationOutcome::Passed)
        }) {
            return Err(ContractError::CandidateRequiredCaseFailed);
        }
        if self.evaluation.cost_micros > manifest.limits.maximum_cost_micros {
            return Err(ContractError::CandidateCostExceeded);
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    PublicBase,
    OrganizationPolicy,
    PersonalKit,
    ProjectLoadout,
    TargetOverrides,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMetadata {
    pub path: PortableSourcePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<GitRevision>,
    pub content_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerDocument {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub kind: LayerKind,
    pub source: SourceMetadata,
    pub spec: Value,
}

/// Closed top-level vocabulary for a CommonKit v1 layer. Provider-specific
/// payloads remain nested below these owned capability and adapter envelopes.
pub const V1_LAYER_SPEC_FIELDS: &[&str] = &[
    "adapters",
    "arguments",
    "capabilities",
    "contextBudget",
    "credentials",
    "databases",
    "denials",
    "files",
    "hooks",
    "plugins",
    "relay",
    "requirements",
    "schedules",
    "securityPolicy",
    "services",
    "settings",
    "snapshots",
    "targets",
    "theme",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecurityPolicy {
    pub denied_paths: BTreeSet<String>,
    pub required_controls: BTreeMap<StableId, bool>,
    pub allowlists: BTreeMap<StableId, BTreeSet<String>>,
    pub minimums: BTreeMap<StableId, i64>,
    pub maximums: BTreeMap<StableId, i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorEnvelope {
    pub schema_version: SchemaVersion,
    pub error: CommonKitErrorBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommonKitErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub category: ErrorCategory,
    pub retryable: bool,
    pub details: BTreeMap<String, Value>,
    pub causes: Vec<ErrorCause>,
    pub remediation: Vec<Remediation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<StableId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Configuration,
    Policy,
    Path,
    Target,
    Plan,
    Apply,
    Verify,
    Credential,
    Relay,
    Snapshot,
    Api,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ErrorCode {
    #[serde(rename = "CK_CONFIG_001_INVALID_DOCUMENT")]
    ConfigInvalidDocument,
    #[serde(rename = "CK_CONFIG_002_UNSUPPORTED_SCHEMA")]
    ConfigUnsupportedSchema,
    #[serde(rename = "CK_CONFIG_004_DUPLICATE_ID")]
    ConfigDuplicateId,
    #[serde(rename = "CK_CONFIG_005_LAYER_CARDINALITY")]
    ConfigLayerCardinality,
    #[serde(rename = "CK_CONFIG_007_INVALID_SOURCE_PATH")]
    ConfigInvalidSourcePath,
    #[serde(rename = "CK_CONFIG_008_DIGEST_MISMATCH")]
    ConfigDigestMismatch,
    #[serde(rename = "CK_POLICY_007_EMBEDDED_SECRET")]
    PolicyEmbeddedSecret,
    #[serde(rename = "CK_PLAN_006_CONFIRMATION_REQUIRED")]
    PlanConfirmationRequired,
    #[serde(rename = "CK_PLAN_007_REDACTION_FAILED")]
    PlanRedactionFailed,
    #[serde(rename = "CK_INTERNAL_001_UNEXPECTED")]
    InternalUnexpected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorCause {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Remediation {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeOperation {
    Set,
    Replace,
    RecursiveMerge,
    Union,
    MergeById,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contribution {
    pub layer_id: StableId,
    pub layer_kind: LayerKind,
    pub source: SourceMetadata,
    pub operation: MergeOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_digest: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceEntry {
    pub winner: Contribution,
    pub contributions: Vec<Contribution>,
    pub governing_rules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvenanceTrace {
    pub schema_version: SchemaVersion,
    pub state_digest: Sha256Digest,
    pub entries: BTreeMap<String, TraceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedLayer {
    pub id: StableId,
    pub kind: LayerKind,
    pub path: PortableSourcePath,
    pub schema_version: SchemaVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<GitRevision>,
    pub content_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommonKitLock {
    pub schema_version: SchemaVersion,
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_revision: Option<String>,
    pub layers: Vec<LockedLayer>,
    pub normalized_digest: Sha256Digest,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Create,
    Update,
    Delete,
    Merge,
    Enable,
    Disable,
    Restart,
    Snapshot,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    Low,
    Medium,
    High,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceRef {
    pub resource_type: StableId,
    pub resource_id: StableId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationProvenance {
    pub provider_id: StableId,
    pub provider_version: String,
    pub input_digest: Sha256Digest,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub id: Sha256Digest,
    pub adapter_id: StableId,
    pub kind: OperationKind,
    pub resource: ResourceRef,
    pub risk: Risk,
    pub requires_confirmation: bool,
    pub depends_on: Vec<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<Sha256Digest>,
    pub payload_digest: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<OperationProvenance>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanBindings {
    pub target_identity_digest: Sha256Digest,
    pub composed_loadout_digest: Sha256Digest,
    pub provider_inputs_digest: Sha256Digest,
    pub ownership_map_digest: Sha256Digest,
    pub artifact_set_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub schema_version: SchemaVersion,
    pub contract_version: String,
    pub id: Sha256Digest,
    pub target_id: StableId,
    pub desired_digest: Sha256Digest,
    pub observed_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub bindings: PlanBindings,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Prepared,
    Applying,
    Verifying,
    Succeeded,
    RecoveryRequired,
    RollingBack,
    RolledBack,
    RollbackFailed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Prepared,
    PrepareFailed,
    ApplyStarted,
    Applied,
    ApplyFailed,
    Verified,
    VerifyFailed,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationProgress {
    pub operation_id: Sha256Digest,
    pub phase: OperationPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptTransition {
    pub sequence: u64,
    pub state: ReceiptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_digest: Option<Sha256Digest>,
    pub progress_digest: Sha256Digest,
    pub entry_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunReceipt {
    pub schema_version: SchemaVersion,
    pub contract_version: String,
    pub receipt_id: Sha256Digest,
    pub run_id: StableId,
    pub plan_id: Sha256Digest,
    pub target_id: StableId,
    pub desired_digest: Sha256Digest,
    pub observed_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub bindings: PlanBindings,
    pub state: ReceiptState,
    pub operation_progress: Vec<OperationProgress>,
    pub transitions: Vec<ReceiptTransition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticState {
    Healthy,
    Drifted,
    Blocked,
    Applying,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDiagnostic {
    pub version: String,
    pub operating_system: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentDiagnostic {
    pub id: StableId,
    pub state: DiagnosticState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticBundle {
    pub schema_version: SchemaVersion,
    pub contract_version: String,
    pub generated_at_unix_ms: u64,
    pub runtime: RuntimeDiagnostic,
    pub overall_state: DiagnosticState,
    pub components: Vec<ComponentDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Assigned,
    Preparing,
    Running,
    Checkpointing,
    Succeeded,
    Failed,
    Canceled,
    Interrupted,
}

impl JobState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Canceled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    Deny,
    Restricted,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceRequirements {
    pub cpu_millis: u32,
    pub memory_mib: u64,
    pub disk_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub retryable_exit_codes: BTreeSet<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactPolicy {
    pub globs: Vec<String>,
    pub retention_seconds: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserProfile {
    pub chromium_revision: String,
    pub operating_system: String,
    pub fonts_digest: Sha256Digest,
    pub locale: String,
    pub timezone: String,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub device_scale_factor_milli: u32,
    pub headless: bool,
    pub performance_class: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionManifest {
    pub schema_version: SchemaVersion,
    pub repository: String,
    pub repository_revision: GitRevision,
    pub workspace_bundle_digest: Option<Sha256Digest>,
    pub argv: Vec<String>,
    pub workdir: PortableSourcePath,
    pub secret_refs: Vec<String>,
    pub timeout_seconds: u64,
    pub cancel_grace_seconds: u64,
    pub resources: ResourceRequirements,
    pub required_capabilities: BTreeSet<StableId>,
    pub loadout_digest: Sha256Digest,
    pub execution_profile_digest: Sha256Digest,
    pub retry: RetryPolicy,
    pub checkpoint_enabled: bool,
    pub artifacts: ArtifactPolicy,
    pub network_policy: NetworkPolicy,
    pub repository_write: bool,
    pub browser: Option<BrowserProfile>,
}

impl ExecutionManifest {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.argv.is_empty() {
            return Err(ContractError::InvalidExecutionManifest(
                "argv must not be empty".into(),
            ));
        }
        if self.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(ContractError::InvalidExecutionManifest(
                "argv contains NUL".into(),
            ));
        }
        if self.secret_refs.iter().any(|value| !is_reference(value)) {
            return Err(ContractError::InvalidExecutionManifest(
                "secretRefs must contain references only".into(),
            ));
        }
        let value = serde_json::to_value(self).map_err(|_| ContractError::Canonicalization)?;
        assert_no_embedded_secrets(&value)
    }
    pub fn digest(&self) -> Result<Sha256Digest, ContractError> {
        self.validate()?;
        digest_domain_json("commonkit.execution-manifest.v1", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Job {
    pub id: String,
    pub manifest_digest: Sha256Digest,
    pub manifest: ExecutionManifest,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    pub job_id: String,
    pub number: u32,
    pub revision: u64,
    pub state: JobState,
    pub target_id: Option<StableId>,
    pub engine_run_ref: Option<String>,
    pub fencing_token: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobEvent {
    pub id: String,
    pub job_id: String,
    pub sequence: u64,
    pub attempt_id: Option<String>,
    pub kind: String,
    pub at_unix_ms: u64,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lease {
    pub job_id: String,
    pub attempt_id: String,
    pub worker_id: StableId,
    pub fencing_token: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checkpoint {
    pub id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub stage: String,
    pub fencing_token: u64,
    pub object_digest: Sha256Digest,
    pub committed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub name: String,
    pub object_digest: Sha256Digest,
    pub size_bytes: u64,
    pub media_type: String,
    pub committed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTarget {
    pub id: StableId,
    pub platform: String,
    pub healthy: bool,
    pub draining: bool,
    pub loadout_digest: Sha256Digest,
    pub execution_profile_digest: Sha256Digest,
    pub capabilities: BTreeSet<StableId>,
    pub ready_secret_refs: BTreeSet<String>,
    pub free: ResourceRequirements,
    pub queue_depth: u32,
    pub cost_score: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementExplanation {
    pub target_id: StableId,
    pub eligible: bool,
    pub reasons: Vec<String>,
    pub score: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionReceipt {
    pub schema_version: SchemaVersion,
    pub job_id: String,
    pub attempt_id: String,
    pub state: JobState,
    pub exit_code: Option<i32>,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
    pub artifact_ids: Vec<String>,
    pub checkpoint_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserShard {
    pub shard_id: String,
    pub ordinal: u32,
    pub task_id: String,
}

pub fn layer_schema() -> Result<Value, ContractError> {
    let mut schema = serde_json::to_value(schema_for!(LayerDocument))
        .map_err(|_| ContractError::SchemaGeneration)?;
    let object = schema
        .as_object_mut()
        .ok_or(ContractError::SchemaGeneration)?;
    object.insert(
        "$id".into(),
        Value::String("https://schemas.commonkit.dev/v1/layer.schema.json".into()),
    );
    let spec = schema
        .pointer_mut("/properties/spec")
        .ok_or(ContractError::SchemaGeneration)?;
    *spec = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": V1_LAYER_SPEC_FIELDS
            .iter()
            .map(|field| ((*field).to_owned(), Value::Bool(true)))
            .collect::<Map<String, Value>>()
    });
    for (path, pattern) in [
        ("/properties/id", r"^[a-z][a-z0-9_-]{0,62}$"),
        (
            "/$defs/SourceMetadata/properties/contentDigest",
            r"^sha256:[0-9a-f]{64}$",
        ),
        (
            "/$defs/SourceMetadata/properties/path",
            r"^(?!/)(?!.*(?:^|/)\.\.?/)(?!.*[\\:\x00])[^/]+(?:/[^/]+)*$",
        ),
        (
            "/$defs/SourceMetadata/properties/revision",
            r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$",
        ),
    ] {
        let schema = schema
            .pointer_mut(path)
            .and_then(Value::as_object_mut)
            .ok_or(ContractError::SchemaGeneration)?;
        schema.insert("pattern".into(), Value::String(pattern.into()));
    }
    Ok(schema)
}

pub fn error_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(ErrorEnvelope),
        "https://schemas.commonkit.dev/v1/error.schema.json",
    )
}

pub fn provenance_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(ProvenanceTrace),
        "https://schemas.commonkit.dev/v1/provenance.schema.json",
    )
}

pub fn lock_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(CommonKitLock),
        "https://schemas.commonkit.dev/v1/commonkit-lock.schema.json",
    )
}

pub fn plan_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(Plan),
        "https://schemas.commonkit.dev/v1/plan.schema.json",
    )
}

pub fn receipt_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(RunReceipt),
        "https://schemas.commonkit.dev/v1/receipt.schema.json",
    )
}

pub fn diagnostics_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(DiagnosticBundle),
        "https://schemas.commonkit.dev/v1/diagnostics.schema.json",
    )
}

/// Public entry point for the versioned CommonKit document family.
///
/// Individual contracts remain independently addressable so tools can select
/// the narrowest schema, while this catalog gives editors and registries one
/// stable v1 schema URL.
pub fn commonkit_schema() -> Result<Value, ContractError> {
    Ok(serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://schemas.commonkit.dev/v1/commonkit.schema.json",
        "title": "CommonKit v1 public contracts",
        "description": "A CommonKit layer, lock, plan, receipt, diagnostic, error, provenance, or skill lifecycle document.",
        "oneOf": [
            { "$ref": "layer.schema.json" },
            { "$ref": "commonkit-lock.schema.json" },
            { "$ref": "plan.schema.json" },
            { "$ref": "receipt.schema.json" },
            { "$ref": "diagnostics.schema.json" },
            { "$ref": "error.schema.json" },
            { "$ref": "provenance.schema.json" },
            { "$ref": "skills.schema.json" }
        ]
    }))
}

pub fn skills_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(SkillLifecycleSchema),
        "https://schemas.commonkit.dev/v1/skills.schema.json",
    )
}

pub fn execution_manifest_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(ExecutionManifest),
        "https://schemas.commonkit.dev/v1/execution-manifest.schema.json",
    )
}

pub fn execution_receipt_schema() -> Result<Value, ContractError> {
    schema_with_id(
        schema_for!(ExecutionReceipt),
        "https://schemas.commonkit.dev/v1/execution-receipt.schema.json",
    )
}

fn schema_with_id(schema: schemars::Schema, id: &str) -> Result<Value, ContractError> {
    let mut schema = serde_json::to_value(schema).map_err(|_| ContractError::SchemaGeneration)?;
    schema
        .as_object_mut()
        .ok_or(ContractError::SchemaGeneration)?
        .insert("$id".into(), Value::String(id.into()));
    Ok(schema)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("JSON canonicalization failed")]
    Canonicalization,
    #[error("JSON Schema generation failed")]
    SchemaGeneration,
    #[error("stable ID must match ^[a-z][a-z0-9_-]{{0,62}}$: {0}")]
    InvalidStableId(String),
    #[error("SHA-256 digest must match ^sha256:[0-9a-f]{{64}}$: {0}")]
    InvalidSha256Digest(String),
    #[error("source path must be a repository-relative POSIX path: {0}")]
    InvalidPortableSourcePath(String),
    #[error("Git revision must be a full lowercase 40- or 64-character hexadecimal object ID: {0}")]
    InvalidGitRevision(String),
    #[error("embedded secret-like value at {0}")]
    EmbeddedSecret(String),
    #[error("evaluation case {0} appears in more than one split")]
    EvaluationCaseOverlap(String),
    #[error("evaluation train, validation, and held-out splits must be non-empty")]
    EmptyEvaluationSplit,
    #[error("required evaluation cases must belong to the held-out split")]
    RequiredCaseOutsideHeldOut,
    #[error("{0} must be a bounded non-empty single-line value")]
    InvalidLockText(&'static str),
    #[error("an optimizable skill must declare at least one target")]
    MissingSkillTarget,
    #[error("optimization limits must be positive")]
    InvalidOptimizationLimits,
    #[error("unsupported SkillOpt provider version or compatibility contract")]
    UnsupportedProviderVersion,
    #[error("candidate manifest digest does not match")]
    CandidateManifestMismatch,
    #[error("candidate parent skill digest does not match")]
    CandidateParentMismatch,
    #[error("candidate provider or configuration provenance does not match")]
    CandidateProviderMismatch,
    #[error("candidate input digests do not match")]
    CandidateInputMismatch,
    #[error("candidate harness does not match the evaluation suite")]
    CandidateHarnessMismatch,
    #[error("candidate failed content or policy validation")]
    CandidatePolicyFailed,
    #[error("candidate improvement is below the required threshold")]
    CandidateInsufficientImprovement,
    #[error("candidate regressed on the held-out split")]
    CandidateHeldOutRegression,
    #[error("candidate failed a required held-out case")]
    CandidateRequiredCaseFailed,
    #[error("candidate evaluation exceeded its cost limit")]
    CandidateCostExceeded,
    #[error("invalid execution manifest: {0}")]
    InvalidExecutionManifest(String),
}

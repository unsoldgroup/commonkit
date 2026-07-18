//! Stable, side-effect-free CommonKit wire contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::{fmt, sync::OnceLock};

use regex::Regex;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
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
                    if is_secret_key(key) && !is_empty_or_false(value) {
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
}

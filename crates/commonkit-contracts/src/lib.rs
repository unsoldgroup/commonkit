//! Stable, side-effect-free CommonKit wire contracts.

use std::collections::BTreeMap;
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
    pub revision: Option<String>,
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
    #[error("embedded secret-like value at {0}")]
    EmbeddedSecret(String),
}

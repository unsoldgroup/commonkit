//! Stable, side-effect-free CommonKit wire contracts.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const CONTRACT_VERSION: &str = "1.0";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    PublicBase,
    OrganizationPolicy,
    PersonalKit,
    ProjectLoadout,
    TargetOverrides,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMetadata {
    pub path: PortableSourcePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub content_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerDocument {
    pub schema_version: SchemaVersion,
    pub id: StableId,
    pub kind: LayerKind,
    pub source: SourceMetadata,
    pub spec: Value,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("stable ID must match ^[a-z][a-z0-9_-]{{0,62}}$: {0}")]
    InvalidStableId(String),
    #[error("SHA-256 digest must match ^sha256:[0-9a-f]{{64}}$: {0}")]
    InvalidSha256Digest(String),
    #[error("source path must be a repository-relative POSIX path: {0}")]
    InvalidPortableSourcePath(String),
}

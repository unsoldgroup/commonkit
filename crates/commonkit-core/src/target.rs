use std::collections::BTreeSet;

use commonkit_contracts::{ContractError, Sha256Digest, StableId, digest_domain_json};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingSystem {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseSensitivity {
    Sensitive,
    Insensitive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformFacts {
    pub operating_system: OperatingSystem,
    pub architecture: String,
    pub case_sensitivity: CaseSensitivity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetTransport {
    Local {
        machine_id: String,
    },
    Ssh {
        host: String,
        port: u16,
        user: String,
        host_key_fingerprint: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetRoot {
    pub id: StableId,
    pub path: String,
    pub access: RootAccess,
}

impl TargetRoot {
    pub fn parse(
        id: impl Into<String>,
        path: impl Into<String>,
        access: RootAccess,
    ) -> Result<Self, TargetInventoryError> {
        let path = path.into();
        validate_absolute_path(&path)?;
        Ok(Self {
            id: StableId::parse(id.into())?,
            path,
            access,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetInventory {
    id: StableId,
    transport: TargetTransport,
    platform: PlatformFacts,
    roots: Vec<TargetRoot>,
}

impl TargetInventory {
    pub fn new(
        id: StableId,
        transport: TargetTransport,
        platform: PlatformFacts,
        mut roots: Vec<TargetRoot>,
    ) -> Result<Self, TargetInventoryError> {
        validate_nonempty("architecture", &platform.architecture)?;
        match &transport {
            TargetTransport::Local { machine_id } => validate_nonempty("machineId", machine_id)?,
            TargetTransport::Ssh {
                host,
                port,
                user,
                host_key_fingerprint,
            } => {
                validate_nonempty("host", host)?;
                validate_nonempty("user", user)?;
                validate_nonempty("hostKeyFingerprint", host_key_fingerprint)?;
                if *port == 0 {
                    return Err(TargetInventoryError::InvalidField("port"));
                }
            }
        }
        if roots.is_empty() {
            return Err(TargetInventoryError::NoRoots);
        }
        for root in &roots {
            validate_absolute_path(&root.path)?;
        }
        roots.sort();
        let mut ids = BTreeSet::new();
        if let Some(duplicate) = roots.iter().find(|root| !ids.insert(root.id.clone())) {
            return Err(TargetInventoryError::DuplicateRoot(duplicate.id.clone()));
        }
        Ok(Self {
            id,
            transport,
            platform,
            roots,
        })
    }

    pub fn id(&self) -> &StableId {
        &self.id
    }

    pub fn transport(&self) -> &TargetTransport {
        &self.transport
    }

    pub fn platform(&self) -> &PlatformFacts {
        &self.platform
    }

    pub fn roots(&self) -> &[TargetRoot] {
        &self.roots
    }

    pub fn root(&self, id: &StableId) -> Option<&TargetRoot> {
        self.roots.iter().find(|root| &root.id == id)
    }

    pub fn identity_digest(&self) -> Result<Sha256Digest, ContractError> {
        digest_domain_json("commonkit.target-identity.v1", self)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetInventoryWire {
    id: StableId,
    transport: TargetTransport,
    platform: PlatformFacts,
    roots: Vec<TargetRoot>,
}

impl<'de> Deserialize<'de> for TargetInventory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = TargetInventoryWire::deserialize(deserializer)?;
        Self::new(wire.id, wire.transport, wire.platform, wire.roots)
            .map_err(serde::de::Error::custom)
    }
}

fn validate_nonempty(field: &'static str, value: &str) -> Result<(), TargetInventoryError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        Err(TargetInventoryError::InvalidField(field))
    } else {
        Ok(())
    }
}

fn validate_absolute_path(path: &str) -> Result<(), TargetInventoryError> {
    let unix_absolute = path.starts_with('/');
    let windows_absolute = path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().get(2) == Some(&b'/')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    let segments = if windows_absolute {
        &path[3..]
    } else {
        &path[1..]
    };
    if (!unix_absolute && !windows_absolute)
        || path == "/"
        || path.contains('\\')
        || path.contains('\0')
        || segments
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        Err(TargetInventoryError::UnsafeRoot(path.into()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TargetInventoryError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("target inventory field is empty or invalid: {0}")]
    InvalidField(&'static str),
    #[error("target inventory must declare at least one root")]
    NoRoots,
    #[error("target inventory contains a duplicate root ID: {0}")]
    DuplicateRoot(StableId),
    #[error("target root must be a canonical absolute portable path: {0}")]
    UnsafeRoot(String),
}

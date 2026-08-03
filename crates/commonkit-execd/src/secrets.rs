//! Target-local secret resolution.
//!
//! Manifests carry `env://NAME` references, never values. The operator renders the
//! values onto the target out of band; this module reads that file, refuses it when
//! other users can read it, and rejects references a job could never use.

use std::collections::BTreeMap;
use std::path::Path;

/// The environment variable a secret reference names, if the reference is usable.
///
/// Reserved names are rejected so a manifest cannot overwrite the process
/// environment the supervisor relies on, or shadow daemon configuration.
pub fn environment_name(reference: &str) -> Option<&str> {
    reference
        .strip_prefix("env://")
        .filter(|name| valid_environment_name(name) && !reserved_environment_name(name))
}

/// Reads resolved secret values keyed by their `env://NAME` reference.
pub fn load(path: &Path) -> Result<BTreeMap<String, String>, SecretsError> {
    let metadata = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(SecretsError::PermissionsTooOpen(mode));
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    let resolved: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(path)?)?;
    for reference in resolved.keys() {
        if environment_name(reference).is_none() {
            return Err(SecretsError::UnsupportedReference(reference.clone()));
        }
    }
    Ok(resolved)
}

fn valid_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn reserved_environment_name(name: &str) -> bool {
    matches!(name, "PATH" | "LANG" | "LC_ALL" | "HOME" | "TMPDIR")
        || name.starts_with("COMMONKIT_EXECD_")
}

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("secret file is readable by other users: mode {0:o}")]
    PermissionsTooOpen(u32),
    #[error("secret reference is unsupported: {0}")]
    UnsupportedReference(String),
    // Values must never reach an error message, so the payload is dropped.
    #[error("secret file is not a JSON object of reference to value")]
    Malformed,
    #[error("secret file could not be read")]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for SecretsError {
    fn from(_: serde_json::Error) -> Self {
        Self::Malformed
    }
}

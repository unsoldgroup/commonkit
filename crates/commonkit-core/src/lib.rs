//! CommonKit composition, policy, and planning primitives.

use std::sync::OnceLock;

pub use commonkit_contracts::*;
use regex::Regex;
use thiserror::Error;

pub fn is_forbidden_path(candidate: &str) -> bool {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            [
                r"(^|/)auth\.json$",
                r"(^|/)hosts\.yml$",
                r"(?i)(^|/)credentials\.enc$",
                r"(?i)(^|/).*credentials.*\.json$",
                r"(?i)(^|/)client_secret.*\.json$",
                r"(?i)(^|/)token_cache\.json$",
                r"(?i)(^|/)\.encryption_key$",
                r"(^|/)\.env(?:\.|$)",
                r"(?i)(^|/)(?:credentials?|tokens?|secrets?)(?:/|$|\.(?:json|ya?ml|txt|enc)$)",
                r"(?:^|/)orchestration\.db(?:-(?:wal|shm))?$",
                r"(?:^|/)(?:state|logs|memories|goals)_\d+\.sqlite(?:-(?:wal|shm))?$",
                r"(?:^|/)(?:state|logs|memories|goals)(?:_[0-9]+)?\.(?:db|sqlite|sqlite3)(?:-(?:wal|shm))?$",
                r"(?:^|/)context-mode/sessions/",
                r"(?:^|/)orca-(?:devices|e2ee-keypair)\.json$",
                r"(?:^|/)(?:Cookies|Local Storage|Singleton[^/]*)$",
                r"\.(?:sock|token)$",
                r"(?i)\.(?:pem|key)$",
            ]
            .into_iter()
            .map(|pattern| Regex::new(pattern).expect("forbidden-path regex"))
            .collect()
        })
        .iter()
        .any(|pattern| pattern.is_match(candidate))
}

pub fn enforce_policy_floor(
    organization: &SecurityPolicy,
    effective: &SecurityPolicy,
) -> Result<(), PolicyViolation> {
    for denied_path in &organization.denied_paths {
        if !effective.denied_paths.contains(denied_path) {
            return Err(PolicyViolation::DenialRemoved {
                value: denied_path.clone(),
            });
        }
    }
    for (control, required) in &organization.required_controls {
        if *required && effective.required_controls.get(control) != Some(&true) {
            return Err(PolicyViolation::RequiredControlWeakened {
                control: control.clone(),
            });
        }
    }
    for (name, allowed) in &organization.allowlists {
        let Some(effective_allowed) = effective.allowlists.get(name) else {
            return Err(PolicyViolation::AllowlistWidened { name: name.clone() });
        };
        if !effective_allowed.is_subset(allowed) {
            return Err(PolicyViolation::AllowlistWidened { name: name.clone() });
        }
    }
    for (name, minimum) in &organization.minimums {
        if effective
            .minimums
            .get(name)
            .is_none_or(|value| value < minimum)
        {
            return Err(PolicyViolation::MinimumLowered { name: name.clone() });
        }
    }
    for (name, maximum) in &organization.maximums {
        if effective
            .maximums
            .get(name)
            .is_none_or(|value| value > maximum)
        {
            return Err(PolicyViolation::MaximumRaised { name: name.clone() });
        }
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyViolation {
    #[error("organization denial was removed: {value}")]
    DenialRemoved { value: String },
    #[error("required organization control was weakened: {control}")]
    RequiredControlWeakened { control: StableId },
    #[error("organization allowlist was widened: {name}")]
    AllowlistWidened { name: StableId },
    #[error("organization minimum was lowered: {name}")]
    MinimumLowered { name: StableId },
    #[error("organization maximum was raised: {name}")]
    MaximumRaised { name: StableId },
}

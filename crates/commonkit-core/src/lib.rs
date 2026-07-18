//! CommonKit composition, policy, and planning primitives.

use std::sync::OnceLock;

pub use commonkit_contracts::*;
use regex::Regex;

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

//! Context budget ledger for a composed loadout.
//!
//! A loadout's context budget is the agent context a target pays for on every
//! turn. It splits into two independently reducible parts:
//!
//! * **Always-on text** — compiled agent instructions (`AGENTS.md`,
//!   `CLAUDE.md`) and hook prose. Injected regardless of task, so it is
//!   reducible only by rewriting it.
//! * **Router text** — the frontmatter block of each `SKILL.md`, which is what
//!   an agent reads to decide whether to load the skill. Its cost scales with
//!   the number of admitted skills, not their size, so it is reducible only by
//!   admitting fewer skills.
//!
//! A skill's body is measured and reported but never charged: it enters an
//! agent's context only when that skill is actually loaded.
//!
//! Measurement is intentionally read-only and operates on staged normalized
//! resources, so a ledger can be produced for a loadout that has not been
//! applied to any target.

use std::collections::BTreeMap;

use commonkit_contracts::{Plan, Sha256Digest};
use serde::Serialize;

use crate::artifacts::{ArtifactError, ArtifactStore};
use crate::resources::{FilesystemIntent, NormalizedResource};

/// Bytes of text assumed to occupy one model token.
///
/// The ledger deliberately does not depend on a model-specific tokenizer. A
/// fixed divisor keeps measurement deterministic, offline, and comparable
/// across models, which is what budget review needs; absolute fidelity to any
/// one tokenizer is not.
//
// ponytail: fixed bytes-per-token divisor. Swap in a real BPE tokenizer only
// if budget decisions start turning on differences smaller than its error.
pub const BYTES_PER_TOKEN: u64 = 4;

/// Converts a byte count to the ledger's token estimate.
///
/// Rounds up so that any non-empty content costs at least one token.
pub fn tokens_for_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(BYTES_PER_TOKEN)
}

/// How a piece of managed content is charged against the context budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextClass {
    /// Injected on every turn. Charged.
    AlwaysOn,
    /// One routing entry per skill, injected on every turn. Charged.
    Router,
    /// Loaded only on demand. Measured and reported, never charged.
    OnDisk,
}

impl ContextClass {
    /// Whether content in this class counts against the budget limit.
    pub fn is_charged(self) -> bool {
        matches!(self, Self::AlwaysOn | Self::Router)
    }
}

/// One measured contribution to the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetEntry {
    pub path: String,
    pub class: ContextClass,
    pub bytes: u64,
    pub tokens: u64,
}

/// The measured context cost of a composed loadout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudgetLedger {
    /// Every measured contribution, ordered by path then class.
    pub entries: Vec<BudgetEntry>,
    pub always_on_tokens: u64,
    pub router_tokens: u64,
    /// Reported for visibility. Never counted toward `charged_tokens`.
    pub on_disk_tokens: u64,
    /// Number of skills contributing router text.
    pub skill_count: usize,
    /// Declared limit, if the loadout sets one. `None` means report-only.
    pub limit: Option<u64>,
}

impl ContextBudgetLedger {
    /// Tokens charged against the budget on every turn.
    pub fn charged_tokens(&self) -> u64 {
        self.always_on_tokens.saturating_add(self.router_tokens)
    }

    /// Whether the charged total exceeds a declared limit.
    ///
    /// Always false when no limit is declared: an undeclared budget reports
    /// figures and never blocks.
    pub fn over_limit(&self) -> bool {
        self.limit
            .is_some_and(|limit| self.charged_tokens() > limit)
    }

    /// Tokens still available, when a limit is declared.
    pub fn remaining_tokens(&self) -> Option<u64> {
        self.limit
            .map(|limit| limit.saturating_sub(self.charged_tokens()))
    }

    /// Measures staged resources into a ledger.
    ///
    /// Resources that are not agent context — and every non-file intent — are
    /// ignored rather than rejected, so a ledger can be taken over a full
    /// loadout without filtering it first.
    pub fn measure(
        resources: &[NormalizedResource],
        store: &ArtifactStore,
        limit: Option<u64>,
    ) -> Result<Self, BudgetError> {
        let content = resources.iter().filter_map(|resource| {
            let FilesystemIntent::File { path, content, .. } = &resource.intent else {
                return None;
            };
            Some((path.as_str().to_string(), content.digest.clone()))
        });
        Self::measure_content(content, store, limit)
    }

    /// Measures a durable plan without re-running any provider.
    ///
    /// A plan records each managed path alongside the digest of the content it
    /// would leave there, which is everything the ledger needs. Operations that
    /// leave no content — removals in particular — contribute nothing, so a
    /// plan that evicts a skill measures as the loadout after the eviction.
    pub fn measure_plan(
        plan: &Plan,
        store: &ArtifactStore,
        limit: Option<u64>,
    ) -> Result<Self, BudgetError> {
        let content = plan.operations.iter().filter_map(|operation| {
            let path = operation.resource.managed_path.clone()?;
            let digest = operation.after_digest.clone()?;
            Some((path, digest))
        });
        Self::measure_content(content, store, limit)
    }

    fn measure_content(
        content: impl Iterator<Item = (String, Sha256Digest)>,
        store: &ArtifactStore,
        limit: Option<u64>,
    ) -> Result<Self, BudgetError> {
        // Keyed by (path, class) so a SKILL.md contributing both router and
        // on-disk text lands as two stable, separately reviewable entries.
        let mut measured: BTreeMap<(String, ContextClass), BudgetEntry> = BTreeMap::new();
        let mut skill_paths = std::collections::BTreeSet::new();

        for (path, digest) in content {
            let Some(role) = classify(&path) else {
                continue;
            };

            let bytes = store
                .load_by_digest(&digest)
                .map_err(|source| BudgetError::Content {
                    path: path.clone(),
                    source,
                })?;

            let contributions = match role {
                ContentRole::AlwaysOn => vec![(ContextClass::AlwaysOn, bytes.len() as u64)],
                ContentRole::Skill => {
                    skill_paths.insert(path.clone());
                    let (router, body) = split_skill(&bytes);
                    vec![(ContextClass::Router, router), (ContextClass::OnDisk, body)]
                }
            };

            for (class, byte_count) in contributions {
                let entry = measured
                    .entry((path.clone(), class))
                    .or_insert_with(|| BudgetEntry {
                        path: path.clone(),
                        class,
                        bytes: 0,
                        tokens: 0,
                    });
                entry.bytes = entry.bytes.saturating_add(byte_count);
                entry.tokens = tokens_for_bytes(entry.bytes);
            }
        }

        let entries: Vec<BudgetEntry> = measured.into_values().collect();
        let total = |class: ContextClass| -> u64 {
            entries
                .iter()
                .filter(|entry| entry.class == class)
                .map(|entry| entry.tokens)
                .fold(0, u64::saturating_add)
        };

        Ok(Self {
            always_on_tokens: total(ContextClass::AlwaysOn),
            router_tokens: total(ContextClass::Router),
            on_disk_tokens: total(ContextClass::OnDisk),
            skill_count: skill_paths.len(),
            limit,
            entries,
        })
    }
}

/// Layer spec field holding a loadout's declared context budget.
pub const CONTEXT_BUDGET_FIELD: &str = "contextBudget";

/// Reads the declared token limit from a composed loadout spec.
///
/// Returns `None` when the loadout declares no budget, which means report-only:
/// figures are still measured and reported, and nothing ever blocks.
///
/// A malformed or non-positive declaration is rejected rather than silently
/// treated as absent, so a typo cannot quietly disable a budget someone meant
/// to enforce.
pub fn declared_limit(spec: &serde_json::Value) -> Result<Option<u64>, BudgetError> {
    let Some(budget) = spec.get(CONTEXT_BUDGET_FIELD) else {
        return Ok(None);
    };
    if budget.is_null() {
        return Ok(None);
    }
    let Some(limit) = budget.get("maxTotalTokens") else {
        return Err(BudgetError::MalformedBudget {
            reason: "contextBudget must declare maxTotalTokens",
        });
    };
    match limit.as_u64() {
        Some(0) | None => Err(BudgetError::MalformedBudget {
            reason: "maxTotalTokens must be a positive integer",
        }),
        Some(limit) => Ok(Some(limit)),
    }
}

/// What kind of agent context a managed path holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentRole {
    AlwaysOn,
    Skill,
}

/// Compiled agent instruction files, charged in full on every turn.
const ALWAYS_ON_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

fn classify(path: &str) -> Option<ContentRole> {
    let basename = path.rsplit('/').next().unwrap_or(path);
    if ALWAYS_ON_FILES.contains(&basename) {
        return Some(ContentRole::AlwaysOn);
    }
    if basename == "SKILL.md" {
        return Some(ContentRole::Skill);
    }
    // Hook prose is always-on: it is inlined into the agent's instructions.
    if basename.ends_with(".md") && path.split('/').any(|segment| segment == "hooks") {
        return Some(ContentRole::AlwaysOn);
    }
    None
}

/// Splits a skill file into (router bytes, body bytes).
///
/// The leading YAML frontmatter block is the router surface: it is what an
/// agent reads to decide whether to load the skill, so it is charged. The
/// delimiters themselves are not content and are excluded from both counts.
/// A skill with no frontmatter contributes no router text.
fn split_skill(bytes: &[u8]) -> (u64, u64) {
    let total = bytes.len() as u64;
    let Some(text) = std::str::from_utf8(bytes).ok() else {
        return (0, total);
    };
    let Some(rest) = strip_frontmatter_open(text) else {
        return (0, total);
    };
    // Find the closing delimiter at the start of a line.
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let router = rest[..offset].len() as u64;
            let body = rest[offset + line.len()..].len() as u64;
            return (router, body);
        }
        offset += line.len();
    }
    // Unterminated frontmatter is not a router block; charge nothing.
    (0, total)
}

fn strip_frontmatter_open(text: &str) -> Option<&str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for delimiter in ["---\n", "---\r\n"] {
        if let Some(rest) = text.strip_prefix(delimiter) {
            return Some(rest);
        }
    }
    None
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error("unable to read managed content at {path}")]
    Content {
        path: String,
        #[source]
        source: ArtifactError,
    },
    #[error("invalid contextBudget declaration: {reason}")]
    MalformedBudget { reason: &'static str },
}

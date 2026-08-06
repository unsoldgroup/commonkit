use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextScope {
    Organization,
    Project,
    Personal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateContext {
    pub id: StableId,
    pub section_id: StableId,
    pub scope: ContextScope,
    pub purpose: StableId,
    pub minimum_trust: u8,
    pub priority: i32,
    pub token_cost: u32,
    pub content_hash: Sha256Digest,
    pub conflict: bool,
    pub procedure_gate: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSession {
    pub id: StableId,
    pub user_id: StableId,
    pub organization_id: StableId,
    pub project_id: StableId,
    pub purpose: StableId,
    pub trust_class: u8,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub revocation_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextGrantRule {
    pub id: StableId,
    pub context_id: StableId,
    pub organization_id: StableId,
    pub project_id: StableId,
    pub purpose: StableId,
    pub minimum_trust: u8,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub revocation_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextResolution {
    pub selected: Vec<StableId>,
    pub retrievable: Vec<StableId>,
    pub descriptor_only: Vec<StableId>,
    pub blocked_operations: Vec<StableId>,
    pub receipt_digest: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptAudience {
    User,
    Organization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptSelection {
    pub context_id: StableId,
    pub scope: ContextScope,
    pub content_hash: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptProjection {
    pub public_context_ids: Vec<StableId>,
    pub private_fragment_ref: Option<Sha256Digest>,
}

pub fn project_receipt(
    selections: &[ReceiptSelection],
    audience: ReceiptAudience,
) -> Result<ReceiptProjection, ContextResolverError> {
    let mut public_context_ids = selections
        .iter()
        .filter(|selection| selection.scope != ContextScope::Personal)
        .map(|selection| selection.context_id.clone())
        .collect::<Vec<_>>();
    public_context_ids.sort();
    let private = selections
        .iter()
        .filter(|selection| selection.scope == ContextScope::Personal)
        .collect::<Vec<_>>();
    let private_fragment_ref = match audience {
        ReceiptAudience::Organization => None,
        ReceiptAudience::User if private.is_empty() => None,
        ReceiptAudience::User => Some(
            digest_domain_json("commonkit.context-receipt.private-fragment.v1", &private)
                .map_err(|_| ContextResolverError::Digest)?,
        ),
    };
    Ok(ReceiptProjection {
        public_context_ids,
        private_fragment_ref,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptSemantic<'a> {
    session_id: &'a StableId,
    evaluation_time_unix_ms: u64,
    revocation_generation: u64,
    token_budget: u32,
    ranker_version: &'static str,
    tokenizer_version: &'static str,
    candidate_digest: Sha256Digest,
    selected: &'a [StableId],
    retrievable: &'a [StableId],
    descriptor_only: &'a [StableId],
    blocked_operations: &'a [StableId],
}

pub fn resolve_context(
    session: &RuntimeSession,
    now_unix_ms: u64,
    current_revocation_generation: u64,
    token_budget: u32,
    candidates: &[CandidateContext],
    grants: &[ContextGrantRule],
) -> Result<ContextResolution, ContextResolverError> {
    authorize_session(session, now_unix_ms, current_revocation_generation)?;
    let mut eligible = Vec::new();
    let mut descriptor_only = Vec::new();
    for candidate in candidates {
        if candidate.purpose != session.purpose || candidate.minimum_trust > session.trust_class {
            continue;
        }
        let authorized = candidate.scope != ContextScope::Personal
            || grants.iter().any(|grant| {
                grant.context_id == candidate.id
                    && grant.organization_id == session.organization_id
                    && grant.project_id == session.project_id
                    && grant.purpose == session.purpose
                    && session.trust_class >= grant.minimum_trust
                    && grant.issued_at_unix_ms <= now_unix_ms
                    && now_unix_ms < grant.expires_at_unix_ms
                    && grant.revocation_generation == current_revocation_generation
            });
        if authorized {
            eligible.push(candidate);
        } else {
            descriptor_only.push(candidate.id.clone());
        }
    }
    eligible.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    descriptor_only.sort();

    let mut remaining = token_budget;
    let mut selected = Vec::new();
    let mut retrievable = Vec::new();
    let mut blocked_operations = Vec::new();
    for candidate in eligible {
        if candidate.token_cost <= remaining {
            remaining -= candidate.token_cost;
            selected.push(candidate.id.clone());
        } else {
            retrievable.push(candidate.id.clone());
            if let Some(operation) = &candidate.procedure_gate {
                blocked_operations.push(operation.clone());
            }
        }
    }
    let candidate_digest =
        digest_domain_json("commonkit.context-resolver.candidates.v1", &candidates)
            .map_err(|_| ContextResolverError::Digest)?;
    let receipt_digest = digest_domain_json(
        "commonkit.context-receipt.v1",
        &ReceiptSemantic {
            session_id: &session.id,
            evaluation_time_unix_ms: now_unix_ms,
            revocation_generation: current_revocation_generation,
            token_budget,
            ranker_version: "priority-id-v1",
            tokenizer_version: "declared-cost-v1",
            candidate_digest,
            selected: &selected,
            retrievable: &retrievable,
            descriptor_only: &descriptor_only,
            blocked_operations: &blocked_operations,
        },
    )
    .map_err(|_| ContextResolverError::Digest)?;
    Ok(ContextResolution {
        selected,
        retrievable,
        descriptor_only,
        blocked_operations,
        receipt_digest,
    })
}

fn authorize_session(
    session: &RuntimeSession,
    now_unix_ms: u64,
    current_revocation_generation: u64,
) -> Result<(), ContextResolverError> {
    if session.issued_at_unix_ms > now_unix_ms || now_unix_ms >= session.expires_at_unix_ms {
        return Err(ContextResolverError::ExpiredSession);
    }
    if session.revocation_generation != current_revocation_generation {
        return Err(ContextResolverError::StaleSession);
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContextResolverError {
    #[error("runtime session is expired or not yet valid")]
    ExpiredSession,
    #[error("runtime session has stale revocation evidence")]
    StaleSession,
    #[error("context resolution receipt could not be computed")]
    Digest,
}

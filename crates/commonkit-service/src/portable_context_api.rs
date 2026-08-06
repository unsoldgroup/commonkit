use std::collections::BTreeMap;
use std::sync::Mutex;

use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use commonkit_core::{
    CandidateContext, ContextGrantRule, ContextScope, ReceiptAudience, ReceiptProjection,
    ReceiptSelection, RuntimeSession, project_receipt, resolve_context,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ContextSectionRecord {
    pub candidate: CandidateContext,
    pub title: String,
    pub summary: String,
    /// Decrypted personal content is deliberately runtime-only. Callers must
    /// repopulate it after restart from an authorized encrypted revision.
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextSearchResult {
    pub descriptor_id: StableId,
    pub section_id: StableId,
    pub title: String,
    pub summary: String,
    pub scope: ContextScope,
    pub retrievable: bool,
    pub content_hash: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetrievedContextSection {
    pub section_id: StableId,
    pub content: String,
    pub content_hash: Sha256Digest,
    pub receipt_id: StableId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextAccessRequestRecord {
    pub id: StableId,
    pub session_id: StableId,
    pub descriptor_id: StableId,
    pub purpose: StableId,
    pub expires_at_unix_ms: u64,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRevisionProposalRecord {
    pub id: StableId,
    pub session_id: StableId,
    pub field_ids: Vec<StableId>,
    pub rationale: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
struct StoredReceipt {
    session_id: StableId,
    selections: Vec<ReceiptSelection>,
    digest: Sha256Digest,
}

#[derive(Default)]
struct ContextState {
    sessions: BTreeMap<StableId, RuntimeSession>,
    sections: BTreeMap<StableId, ContextSectionRecord>,
    grants: Vec<ContextGrantRule>,
    receipts: BTreeMap<StableId, StoredReceipt>,
    access_requests: BTreeMap<StableId, ContextAccessRequestRecord>,
    proposals: BTreeMap<StableId, ProfileRevisionProposalRecord>,
    revocation_generation: u64,
    sequence: u64,
}

#[derive(Default)]
pub struct ContextApiStore {
    state: Mutex<ContextState>,
}

impl ContextApiStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_runtime_state(
        &self,
        sessions: Vec<RuntimeSession>,
        sections: Vec<ContextSectionRecord>,
        grants: Vec<ContextGrantRule>,
        revocation_generation: u64,
    ) -> Result<(), ContextApiError> {
        let mut state = self.lock()?;
        state.sessions = sessions
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect();
        state.sections = sections
            .into_iter()
            .map(|item| (item.candidate.section_id.clone(), item))
            .collect();
        state.grants = grants;
        state.revocation_generation = revocation_generation;
        Ok(())
    }

    pub fn search(
        &self,
        session_id: &str,
        query: &str,
        limit: usize,
        now_unix_ms: u64,
    ) -> Result<Vec<ContextSearchResult>, ContextApiError> {
        let state = self.lock()?;
        let session = find_session(&state, session_id)?;
        let candidates = state
            .sections
            .values()
            .map(|section| section.candidate.clone())
            .collect::<Vec<_>>();
        let resolution = resolve_context(
            session,
            now_unix_ms,
            state.revocation_generation,
            0,
            &candidates,
            &state.grants,
        )?;
        let query = query.to_lowercase();
        let mut results = state
            .sections
            .values()
            .filter(|section| {
                section.candidate.purpose == session.purpose
                    && section.candidate.minimum_trust <= session.trust_class
                    && (section.title.to_lowercase().contains(&query)
                        || section.summary.to_lowercase().contains(&query))
            })
            .map(|section| ContextSearchResult {
                descriptor_id: section.candidate.id.clone(),
                section_id: section.candidate.section_id.clone(),
                title: section.title.clone(),
                summary: section.summary.clone(),
                scope: section.candidate.scope,
                retrievable: resolution.retrievable.contains(&section.candidate.id),
                content_hash: section.candidate.content_hash.clone(),
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.descriptor_id.cmp(&right.descriptor_id));
        results.truncate(limit);
        Ok(results)
    }

    pub fn retrieve(
        &self,
        session_id: &str,
        section_id: &str,
        max_bytes: usize,
        now_unix_ms: u64,
    ) -> Result<RetrievedContextSection, ContextApiError> {
        let mut state = self.lock()?;
        let session = find_session(&state, session_id)?.clone();
        let section = state
            .sections
            .values()
            .find(|section| section.candidate.section_id.as_str() == section_id)
            .cloned()
            .ok_or(ContextApiError::UnknownSection)?;
        let resolution = resolve_context(
            &session,
            now_unix_ms,
            state.revocation_generation,
            section.candidate.token_cost,
            std::slice::from_ref(&section.candidate),
            &state.grants,
        )?;
        if !resolution.selected.contains(&section.candidate.id) {
            return Err(ContextApiError::AccessDenied);
        }
        if section.content.len() > max_bytes {
            return Err(ContextApiError::ResponseTooLarge);
        }
        let receipt_id = next_id(&mut state, "context-receipt")?;
        state.receipts.insert(
            receipt_id.clone(),
            StoredReceipt {
                session_id: session.id,
                selections: vec![ReceiptSelection {
                    context_id: section.candidate.id,
                    scope: section.candidate.scope,
                    content_hash: section.candidate.content_hash.clone(),
                }],
                digest: resolution.receipt_digest,
            },
        );
        Ok(RetrievedContextSection {
            section_id: section.candidate.section_id,
            content: section.content,
            content_hash: section.candidate.content_hash,
            receipt_id,
        })
    }

    pub fn request_access(
        &self,
        session_id: &str,
        descriptor_id: &str,
        purpose: StableId,
        duration_seconds: u32,
        now_unix_ms: u64,
    ) -> Result<ContextAccessRequestRecord, ContextApiError> {
        let mut state = self.lock()?;
        let session = find_session(&state, session_id)?.clone();
        authorize(&state, &session, now_unix_ms)?;
        if purpose != session.purpose {
            return Err(ContextApiError::PurposeMismatch);
        }
        let descriptor_purpose = state
            .sections
            .values()
            .find(|section| section.candidate.id.as_str() == descriptor_id)
            .map(|section| section.candidate.purpose.clone())
            .ok_or(ContextApiError::UnknownDescriptor)?;
        if descriptor_purpose != purpose {
            return Err(ContextApiError::PurposeMismatch);
        }
        let id = next_id(&mut state, "access-request")?;
        let request = ContextAccessRequestRecord {
            id: id.clone(),
            session_id: session.id,
            descriptor_id: StableId::parse(descriptor_id)?,
            purpose,
            expires_at_unix_ms: now_unix_ms.saturating_add(u64::from(duration_seconds) * 1_000),
            created_at_unix_ms: now_unix_ms,
        };
        state.access_requests.insert(id, request.clone());
        Ok(request)
    }

    pub fn inspect_receipt(
        &self,
        session_id: &str,
        receipt_id: &str,
        audience: ReceiptAudience,
        now_unix_ms: u64,
    ) -> Result<(Sha256Digest, ReceiptProjection), ContextApiError> {
        let state = self.lock()?;
        let session = find_session(&state, session_id)?;
        authorize(&state, session, now_unix_ms)?;
        let receipt = state
            .receipts
            .iter()
            .find(|(id, _)| id.as_str() == receipt_id)
            .map(|(_, value)| value)
            .ok_or(ContextApiError::UnknownReceipt)?;
        if receipt.session_id != session.id {
            return Err(ContextApiError::AccessDenied);
        }
        Ok((
            receipt.digest.clone(),
            project_receipt(&receipt.selections, audience)?,
        ))
    }

    pub fn propose_revision(
        &self,
        session_id: &str,
        field_ids: Vec<StableId>,
        rationale: String,
        now_unix_ms: u64,
    ) -> Result<ProfileRevisionProposalRecord, ContextApiError> {
        let mut state = self.lock()?;
        let session = find_session(&state, session_id)?.clone();
        authorize(&state, &session, now_unix_ms)?;
        let id = next_id(&mut state, "profile-proposal")?;
        let proposal = ProfileRevisionProposalRecord {
            id: id.clone(),
            session_id: session.id,
            field_ids,
            rationale,
            created_at_unix_ms: now_unix_ms,
        };
        state.proposals.insert(id, proposal.clone());
        Ok(proposal)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ContextState>, ContextApiError> {
        self.state.lock().map_err(|_| ContextApiError::Unavailable)
    }
}

fn find_session<'a>(
    state: &'a ContextState,
    id: &str,
) -> Result<&'a RuntimeSession, ContextApiError> {
    state
        .sessions
        .values()
        .find(|session| session.id.as_str() == id)
        .ok_or(ContextApiError::UnknownSession)
}

fn authorize(
    state: &ContextState,
    session: &RuntimeSession,
    now_unix_ms: u64,
) -> Result<(), ContextApiError> {
    resolve_context(
        session,
        now_unix_ms,
        state.revocation_generation,
        0,
        &[],
        &[],
    )?;
    Ok(())
}

fn next_id(state: &mut ContextState, domain: &str) -> Result<StableId, ContextApiError> {
    state.sequence = state.sequence.saturating_add(1);
    let digest =
        digest_domain_json(domain, &state.sequence).map_err(|_| ContextApiError::Digest)?;
    StableId::parse(format!("{domain}-{}", &digest.as_str()[7..23])).map_err(Into::into)
}

#[derive(Debug, Error)]
pub enum ContextApiError {
    #[error("context runtime is unavailable")]
    Unavailable,
    #[error("runtime session does not exist")]
    UnknownSession,
    #[error("context descriptor does not exist")]
    UnknownDescriptor,
    #[error("context section does not exist")]
    UnknownSection,
    #[error("context receipt does not exist")]
    UnknownReceipt,
    #[error("context access is denied")]
    AccessDenied,
    #[error("context purpose does not match the runtime session")]
    PurposeMismatch,
    #[error("context response exceeds the caller's bound")]
    ResponseTooLarge,
    #[error("context digest could not be computed")]
    Digest,
    #[error(transparent)]
    Resolver(#[from] commonkit_core::ContextResolverError),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
}

use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_core::{
    CandidateContext, ContextGrantRule, ContextScope, ReceiptAudience, ReceiptSelection,
    RuntimeSession, project_receipt, resolve_context,
};

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn candidate(id: &str, scope: ContextScope, priority: i32, tokens: u32) -> CandidateContext {
    CandidateContext {
        id: StableId::parse(id).unwrap(),
        section_id: StableId::parse(format!("section-{id}")).unwrap(),
        scope,
        purpose: StableId::parse("implementation").unwrap(),
        minimum_trust: 2,
        priority,
        token_cost: tokens,
        content_hash: digest('a'),
        conflict: false,
        procedure_gate: None,
    }
}

fn session() -> RuntimeSession {
    RuntimeSession {
        id: StableId::parse("session-1").unwrap(),
        user_id: StableId::parse("user-1").unwrap(),
        organization_id: StableId::parse("org-1").unwrap(),
        project_id: StableId::parse("project-1").unwrap(),
        purpose: StableId::parse("implementation").unwrap(),
        trust_class: 2,
        issued_at_unix_ms: 100,
        expires_at_unix_ms: 200,
        revocation_generation: 4,
    }
}

#[test]
fn resolution_is_deterministic_budgeted_and_exposes_only_descriptors_without_a_grant() {
    let candidates = vec![
        candidate("org-style", ContextScope::Organization, 10, 4),
        candidate("personal-tone", ContextScope::Personal, 50, 6),
        candidate("project-guide", ContextScope::Project, 20, 4),
    ];
    let grant = ContextGrantRule {
        id: StableId::parse("grant-1").unwrap(),
        context_id: StableId::parse("personal-tone").unwrap(),
        organization_id: StableId::parse("org-1").unwrap(),
        project_id: StableId::parse("project-1").unwrap(),
        purpose: StableId::parse("implementation").unwrap(),
        minimum_trust: 2,
        issued_at_unix_ms: 90,
        expires_at_unix_ms: 180,
        revocation_generation: 4,
    };

    let first = resolve_context(&session(), 150, 4, 8, &candidates, &[grant]).unwrap();
    let second = resolve_context(&session(), 150, 4, 8, &candidates, &[]).unwrap();
    assert_eq!(
        first.selected,
        vec![StableId::parse("personal-tone").unwrap()]
    );
    assert_eq!(first.retrievable.len(), 2);
    assert_eq!(
        second.selected,
        vec![
            StableId::parse("project-guide").unwrap(),
            StableId::parse("org-style").unwrap()
        ]
    );
    assert!(
        second
            .descriptor_only
            .contains(&StableId::parse("personal-tone").unwrap())
    );
    assert_ne!(first.receipt_digest, second.receipt_digest);
    assert_eq!(
        first,
        resolve_context(
            &session(),
            150,
            4,
            8,
            &candidates,
            &[ContextGrantRule {
                id: StableId::parse("grant-1").unwrap(),
                context_id: StableId::parse("personal-tone").unwrap(),
                organization_id: StableId::parse("org-1").unwrap(),
                project_id: StableId::parse("project-1").unwrap(),
                purpose: StableId::parse("implementation").unwrap(),
                minimum_trust: 2,
                issued_at_unix_ms: 90,
                expires_at_unix_ms: 180,
                revocation_generation: 4,
            }]
        )
        .unwrap()
    );
}

#[test]
fn every_resolution_reauthorizes_session_and_grant_revocation_generation() {
    let mut expired = session();
    expired.expires_at_unix_ms = 150;
    assert!(resolve_context(&expired, 150, 4, 8, &[], &[]).is_err());
    assert!(resolve_context(&session(), 150, 5, 8, &[], &[]).is_err());
}

#[test]
fn receipt_projection_never_places_personal_selection_in_organization_state() {
    let selections = vec![
        ReceiptSelection {
            context_id: StableId::parse("org-style").unwrap(),
            scope: ContextScope::Organization,
            content_hash: digest('a'),
        },
        ReceiptSelection {
            context_id: StableId::parse("personal-tone").unwrap(),
            scope: ContextScope::Personal,
            content_hash: digest('b'),
        },
    ];
    let organization = project_receipt(&selections, ReceiptAudience::Organization).unwrap();
    let user = project_receipt(&selections, ReceiptAudience::User).unwrap();

    assert_eq!(
        organization.public_context_ids,
        vec![StableId::parse("org-style").unwrap()]
    );
    assert!(organization.private_fragment_ref.is_none());
    assert!(user.private_fragment_ref.is_some());
    assert!(
        !serde_json::to_string(&organization)
            .unwrap()
            .contains("personal-tone")
    );
}

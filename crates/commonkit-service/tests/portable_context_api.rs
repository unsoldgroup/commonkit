use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_core::{
    CandidateContext, ContextGrantRule, ContextScope, ReceiptAudience, RuntimeSession,
};
use commonkit_service::{
    ContextApiStore, ContextSectionRecord, ControlToken, EventHub, ServiceStatus,
    router_with_context,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

fn id(value: &str) -> StableId {
    StableId::parse(value).unwrap()
}
fn digest(_character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap()
}

fn session() -> RuntimeSession {
    RuntimeSession {
        id: id("session-1"),
        user_id: id("user-1"),
        organization_id: id("org-1"),
        project_id: id("project-1"),
        purpose: id("coding"),
        trust_class: 2,
        issued_at_unix_ms: 0,
        expires_at_unix_ms: u64::MAX,
        revocation_generation: 4,
    }
}

fn section(name: &str, scope: ContextScope, content: &str) -> ContextSectionRecord {
    ContextSectionRecord {
        candidate: CandidateContext {
            id: id(&format!("descriptor-{name}")),
            section_id: id(&format!("section-{name}")),
            scope,
            purpose: id("coding"),
            minimum_trust: 1,
            priority: 1,
            token_cost: 10,
            content_hash: digest(name.chars().next().unwrap()),
            conflict: false,
            procedure_gate: None,
        },
        title: format!("{name} guide"),
        summary: format!("How to use {name}"),
        content: content.into(),
    }
}

#[test]
fn personal_descriptors_are_visible_before_grant_but_values_are_not_retrievable() {
    let store = ContextApiStore::new();
    store
        .replace_runtime_state(
            vec![session()],
            vec![
                section("project", ContextScope::Project, "public"),
                section("personal", ContextScope::Personal, "private"),
            ],
            vec![],
            4,
        )
        .unwrap();

    let results = store.search("session-1", "guide", 10, 200).unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|result| result.scope == ContextScope::Personal && !result.retrievable)
    );
    assert!(
        store
            .retrieve("session-1", "section-personal", 100, 200)
            .is_err()
    );
    assert_eq!(
        store
            .retrieve("session-1", "section-project", 100, 200)
            .unwrap()
            .content,
        "public"
    );
}

#[test]
fn a_matching_grant_allows_retrieval_and_receipts_stay_audience_redacted() {
    let store = ContextApiStore::new();
    let grant = ContextGrantRule {
        id: id("grant-1"),
        context_id: id("descriptor-personal"),
        organization_id: id("org-1"),
        project_id: id("project-1"),
        purpose: id("coding"),
        minimum_trust: 2,
        issued_at_unix_ms: 100,
        expires_at_unix_ms: 1_000,
        revocation_generation: 4,
    };
    store
        .replace_runtime_state(
            vec![session()],
            vec![section("personal", ContextScope::Personal, "private")],
            vec![grant],
            4,
        )
        .unwrap();
    let retrieved = store
        .retrieve("session-1", "section-personal", 100, 200)
        .unwrap();
    let (_, organization) = store
        .inspect_receipt(
            "session-1",
            retrieved.receipt_id.as_str(),
            ReceiptAudience::Organization,
            201,
        )
        .unwrap();
    let (_, user) = store
        .inspect_receipt(
            "session-1",
            retrieved.receipt_id.as_str(),
            ReceiptAudience::User,
            201,
        )
        .unwrap();
    assert!(organization.public_context_ids.is_empty());
    assert!(organization.private_fragment_ref.is_none());
    assert!(user.private_fragment_ref.is_some());
}

#[test]
fn every_call_rejects_a_stale_session_and_response_bounds_fail_closed() {
    let store = ContextApiStore::new();
    store
        .replace_runtime_state(
            vec![session()],
            vec![section("project", ContextScope::Project, "oversized")],
            vec![],
            4,
        )
        .unwrap();
    assert!(
        store
            .retrieve("session-1", "section-project", 3, 200)
            .is_err()
    );
    assert!(store.search("session-1", "guide", 10, u64::MAX).is_err());
}

#[tokio::test]
async fn authenticated_control_routes_back_the_mcp_context_surface() {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;

    let temporary = tempfile::tempdir().unwrap();
    let token = ControlToken::load_or_create(&temporary.path().join("token")).unwrap();
    let authorization = format!("Bearer {}", token.expose_for_client());
    let store = Arc::new(ContextApiStore::new());
    store
        .replace_runtime_state(
            vec![session()],
            vec![section("project", ContextScope::Project, "public")],
            vec![],
            4,
        )
        .unwrap();
    let app = router_with_context(
        token,
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:1",
        EventHub::new(8),
        store,
    );
    let response = app
        .oneshot(
            Request::post("/control/v1/context/search")
                .header("host", "127.0.0.1:1")
                .header("authorization", authorization)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"sessionId":"session-1","query":"project","limit":10}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["results"][0]["sectionId"], "section-project");
    assert_eq!(body["results"][0].get("content"), None);
}

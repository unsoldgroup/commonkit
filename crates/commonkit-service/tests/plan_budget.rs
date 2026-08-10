use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_adapters::{ArtifactStore, ContentSensitivity};
use commonkit_contracts::{OperationKind, PlanBindings, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, EventHub, ExecutionResult, PlanExecutor,
    ServiceStatus, router_with_control, router_with_control_and_artifacts,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

const AUTHORITY: &str = "127.0.0.1:3764";

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn temporary_directory(test: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

struct InertExecutor;

impl PlanExecutor for InertExecutor {
    fn execute(&self, _: &commonkit_contracts::Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Succeeded,
            failure_code: None,
        }
    }
}

/// Builds a plan whose operations write the given (path, content) pairs, and
/// stores that content in a fresh artifact store.
fn plan_writing(store: &ArtifactStore, files: &[(&str, &str)]) -> commonkit_contracts::Plan {
    let operations = files
        .iter()
        .enumerate()
        .map(|(index, (path, contents))| {
            let reference = store
                .put(contents.as_bytes(), ContentSensitivity::Portable)
                .expect("put");
            finalize_operation(OperationDraft {
                adapter_id: StableId::parse("files").expect("adapter"),
                kind: OperationKind::Create,
                resource: ResourceRef {
                    resource_type: StableId::parse("file").expect("type"),
                    resource_id: StableId::parse(format!("resource-{index}")).expect("resource"),
                    managed_path: Some((*path).into()),
                },
                risk: Risk::Low,
                requires_confirmation: false,
                recovery_capability: commonkit_contracts::RecoveryCapability::ExactRollback,
                depends_on: vec![],
                before_digest: None,
                after_digest: Some(reference.digest),
                payload_digest: digest('e'),
                provenance: None,
                summary: format!("write {path}"),
            })
            .expect("operation")
        })
        .collect();

    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: PlanBindings {
            target_identity_digest: digest('3'),
            composed_loadout_digest: digest('4'),
            provider_inputs_digest: digest('5'),
            ownership_map_digest: digest('6'),
            artifact_set_digest: digest('7'),
        },
        operations,
    })
    .expect("plan")
}

fn skill(description: &str, body_bytes: usize) -> String {
    format!(
        "---\nname: example\ndescription: {description}\n---\n{}",
        "x".repeat(body_bytes)
    )
}

async fn budget_of(
    application: axum::Router,
    token: &ControlToken,
    plan_id: &Sha256Digest,
) -> (StatusCode, serde_json::Value) {
    let response = application
        .oneshot(
            Request::get(format!("/control/v1/plans/{}/budget", plan_id.as_str()))
                .header(header::HOST, AUTHORITY)
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn plan_budget_reports_always_on_and_router_text_separately() {
    let root = temporary_directory("plan-budget-report");
    let store = ArtifactStore::open(&root).expect("store");
    let plan = plan_writing(
        &store,
        &[
            ("CLAUDE.md", "Be concise and specific in every reply."),
            ("skills/one/SKILL.md", &skill("Does one thing.", 4_000)),
            ("skills/two/SKILL.md", &skill("Does another thing.", 4_000)),
            // Not agent context: must not be charged or reported.
            ("scripts/build.sh", "#!/bin/sh\nexit 0\n"),
        ],
    );

    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(InertExecutor));
    let plan = control.register_plan(plan).expect("register");
    let application = router_with_control_and_artifacts(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        AUTHORITY,
        EventHub::new(8),
        control,
        Arc::new(store),
    );

    let (status, report) = budget_of(application, &token, &plan.id).await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(report["skillCount"], 2);
    assert!(report["alwaysOnTokens"].as_u64().unwrap() > 0);
    assert!(report["routerTokens"].as_u64().unwrap() > 0);
    assert_eq!(
        report["chargedTokens"].as_u64().unwrap(),
        report["alwaysOnTokens"].as_u64().unwrap() + report["routerTokens"].as_u64().unwrap()
    );

    // The 8 KiB of skill bodies is reported but excluded from the charge.
    let on_disk = report["onDiskTokens"].as_u64().unwrap();
    assert!(on_disk > report["chargedTokens"].as_u64().unwrap());

    // No limit declared: report-only, so an overage is impossible.
    assert_eq!(report["limit"], serde_json::Value::Null);
    assert_eq!(report["overLimit"], false);
    assert_eq!(report["remainingTokens"], serde_json::Value::Null);

    // The shell script contributed nothing.
    let paths: Vec<&str> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(!paths.contains(&"scripts/build.sh"));

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn plan_budget_ignores_removals_so_an_eviction_measures_after_itself() {
    let root = temporary_directory("plan-budget-removal");
    let store = ArtifactStore::open(&root).expect("store");
    let kept = store
        .put(skill("Kept.", 100).as_bytes(), ContentSensitivity::Portable)
        .expect("put");

    let keep = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("files").expect("adapter"),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse("kept").expect("resource"),
            managed_path: Some("skills/kept/SKILL.md".into()),
        },
        risk: Risk::Low,
        requires_confirmation: false,
        recovery_capability: commonkit_contracts::RecoveryCapability::ExactRollback,
        depends_on: vec![],
        before_digest: None,
        after_digest: Some(kept.digest),
        payload_digest: digest('e'),
        provenance: None,
        summary: "write kept".into(),
    })
    .expect("operation");

    // A removal leaves no content, so it carries no after_digest.
    let evict = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("files").expect("adapter"),
        kind: OperationKind::Delete,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse("evicted").expect("resource"),
            managed_path: Some("skills/evicted/SKILL.md".into()),
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        recovery_capability: commonkit_contracts::RecoveryCapability::ExactRollback,
        depends_on: vec![],
        before_digest: Some(digest('9')),
        after_digest: None,
        payload_digest: digest('e'),
        provenance: None,
        summary: "evict skill".into(),
    })
    .expect("operation");

    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: PlanBindings {
            target_identity_digest: digest('3'),
            composed_loadout_digest: digest('4'),
            provider_inputs_digest: digest('5'),
            ownership_map_digest: digest('6'),
            artifact_set_digest: digest('7'),
        },
        operations: vec![keep, evict],
    })
    .expect("plan");

    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(InertExecutor));
    let plan = control.register_plan(plan).expect("register");
    let application = router_with_control_and_artifacts(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        AUTHORITY,
        EventHub::new(8),
        control,
        Arc::new(store),
    );

    let (status, report) = budget_of(application, &token, &plan.id).await;
    assert_eq!(status, StatusCode::OK);
    // Only the surviving skill is charged.
    assert_eq!(report["skillCount"], 1);

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn plan_budget_reports_unavailable_without_an_artifact_store() {
    let root = temporary_directory("plan-budget-unavailable");
    let store = ArtifactStore::open(&root).expect("store");
    let plan = plan_writing(&store, &[("CLAUDE.md", "Be concise.")]);

    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(InertExecutor));
    let plan = control.register_plan(plan).expect("register");
    // router_with_control carries no artifact store.
    let application = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        AUTHORITY,
        EventHub::new(8),
        control,
    );

    let (status, report) = budget_of(application, &token, &plan.id).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(report["error"]["code"], "artifacts_unavailable");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn plan_budget_rejects_an_unknown_plan() {
    let root = temporary_directory("plan-budget-unknown");
    let store = ArtifactStore::open(&root).expect("store");

    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(InertExecutor));
    let application = router_with_control_and_artifacts(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        AUTHORITY,
        EventHub::new(8),
        control,
        Arc::new(store),
    );

    let (status, report) = budget_of(application, &token, &digest('f')).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(report["error"]["code"], "plan_not_found");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn plan_budget_requires_authorization() {
    let root = temporary_directory("plan-budget-auth");
    let store = ArtifactStore::open(&root).expect("store");
    let plan = plan_writing(&store, &[("CLAUDE.md", "Be concise.")]);

    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(InertExecutor));
    let plan = control.register_plan(plan).expect("register");
    let application = router_with_control_and_artifacts(
        token,
        Arc::new(RwLock::new(ServiceStatus::default())),
        AUTHORITY,
        EventHub::new(8),
        control,
        Arc::new(store),
    );

    let response = application
        .oneshot(
            Request::get(format!("/control/v1/plans/{}/budget", plan.id.as_str()))
                .header(header::HOST, AUTHORITY)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    std::fs::remove_dir_all(&root).ok();
}

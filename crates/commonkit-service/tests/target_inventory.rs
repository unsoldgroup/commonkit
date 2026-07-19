use std::fs;

use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use commonkit_reconcile::PlanStore;
use commonkit_service::ProductionDomainRegistry;
use commonkit_service::{TargetInventory, TargetRecord, TargetTransport};
use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, DomainFailure, EventHub, ExecutionResult,
    PlanExecutor, ServiceStatus, SyncDomain, router_with_control,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn target(id: &str, transport: TargetTransport) -> TargetRecord {
    TargetRecord {
        id: StableId::parse(id).unwrap(),
        transport,
        identity_digest: digest_domain_json("test.target", &id).unwrap(),
    }
}

#[test]
fn inventory_migrates_a_legacy_single_target_and_persists_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("targets.json");
    let local = target("workstation", TargetTransport::Local);

    let inventory = TargetInventory::open(&state, vec![local.clone()], None).unwrap();
    assert_eq!(inventory.selected(), vec![local.id.clone()]);

    drop(inventory);
    let reopened = TargetInventory::open(&state, vec![local.clone()], None).unwrap();
    assert_eq!(reopened.selected(), vec![local.id]);
}

#[test]
fn inventory_supports_multiple_ssh_targets_and_rejects_unknown_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("targets.json");
    let laptop = target("laptop", TargetTransport::Local);
    let build = target(
        "build-box",
        TargetTransport::Ssh {
            host: "build.internal".into(),
            user: "builder".into(),
            port: 22,
        },
    );
    let staging = target(
        "staging",
        TargetTransport::Ssh {
            host: "staging.internal".into(),
            user: "deploy".into(),
            port: 2222,
        },
    );
    let inventory = TargetInventory::open(
        &state,
        vec![laptop, build.clone(), staging.clone()],
        Some(vec![build.id.clone(), staging.id.clone()]),
    )
    .unwrap();
    assert_eq!(inventory.selected(), vec![build.id, staging.id]);
    assert!(
        inventory
            .select(vec![StableId::parse("missing").unwrap()])
            .is_err()
    );
}

#[test]
fn inventory_rejects_duplicate_ids_and_identity_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("targets.json");
    let first = target("host-a", TargetTransport::Local);
    let duplicate_id = TargetRecord {
        transport: TargetTransport::Ssh {
            host: "a".into(),
            user: "u".into(),
            port: 22,
        },
        ..first.clone()
    };
    assert!(TargetInventory::open(&state, vec![first.clone(), duplicate_id], None).is_err());

    let collision = TargetRecord {
        id: StableId::parse("host-b").unwrap(),
        identity_digest: first.identity_digest.clone(),
        transport: TargetTransport::Local,
    };
    assert!(TargetInventory::open(&state, vec![first, collision], None).is_err());
}

#[test]
fn corrupt_restart_state_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("targets.json");
    fs::write(&state, br#"{"selected":["missing"]}"#).unwrap();
    let error = TargetInventory::open(&state, vec![target("local", TargetTransport::Local)], None)
        .unwrap_err();
    assert!(error.to_string().contains("unknown target"));
}

#[test]
fn target_identity_digest_is_not_optional_or_synthetic() {
    assert!(Sha256Digest::parse("sha256:bad").is_err());
}

struct NeverExecute;
impl PlanExecutor for NeverExecute {
    fn execute(&self, _: &commonkit_contracts::Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: None,
        }
    }
}

struct TargetEcho(&'static str);
impl SyncDomain for TargetEcho {
    fn plan(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Ok(serde_json::json!({"targetId":self.0,"planId":format!("plan-{}", self.0)}))
    }
    fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Ok(serde_json::json!({"targetId":self.0,"state":"healthy"}))
    }
    fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::InvalidRequest)
    }
}

#[tokio::test]
async fn target_routes_are_authenticated_and_selection_requires_confirmation() {
    let temporary = tempfile::tempdir().unwrap();
    let inventory = Arc::new(
        TargetInventory::open(
            temporary.path().join("targets.json"),
            vec![
                target("local", TargetTransport::Local),
                target(
                    "remote",
                    TargetTransport::Ssh {
                        host: "remote".into(),
                        user: "al".into(),
                        port: 22,
                    },
                ),
            ],
            Some(vec![StableId::parse("local").unwrap()]),
        )
        .unwrap(),
    );
    let control = ControlPlane::new(Arc::new(NeverExecute));
    control.set_target_inventory(inventory);
    control
        .set_target_sync_domains(BTreeMap::from([
            (
                StableId::parse("local").unwrap(),
                Arc::new(TargetEcho("local")) as Arc<dyn SyncDomain>,
            ),
            (
                StableId::parse("remote").unwrap(),
                Arc::new(TargetEcho("remote")) as Arc<dyn SyncDomain>,
            ),
        ]))
        .unwrap();
    let token = ControlToken::generate();
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(4),
        control,
    );

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/control/v1/targets")
                .header(header::HOST, "127.0.0.1:3764")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let unconfirmed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/v1/targets/select")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"targets":["remote"],"confirmed":false,"confirmationId":"test-consent"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let unconfirmed_status = unconfirmed.status();
    let unconfirmed_body = to_bytes(unconfirmed.into_body(), 4096).await.unwrap();
    assert_eq!(
        unconfirmed_status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&unconfirmed_body)
    );

    let selected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/v1/targets/select")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"targets":["remote"],"confirmed":true,"confirmationId":"test-consent"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(selected.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["selected"], serde_json::json!(["remote"]));

    let plan = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/v1/targets/remote/sync/plan")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"target-plan"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(plan.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["targetId"], "remote");
}

#[test]
fn production_config_loads_local_and_multiple_ssh_targets_without_a_sync_domain() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let config = root.join("headless.json");
    let local = target("local", TargetTransport::Local);
    let build = target(
        "build",
        TargetTransport::Ssh {
            host: "build.internal".into(),
            user: "al".into(),
            port: 22,
        },
    );
    let staging = target(
        "staging",
        TargetTransport::Ssh {
            host: "staging.internal".into(),
            user: "deploy".into(),
            port: 22,
        },
    );
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "targets": {
                "state": root.join("target-selection.json"),
                "selected": ["build", "staging"],
                "entries": [local, build, staging]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = ProductionDomainRegistry::load(
        &config,
        Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let inventory = registry.targets.unwrap();
    assert_eq!(inventory.targets().len(), 3);
    assert_eq!(
        inventory
            .selected()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["build", "staging"]
    );
}

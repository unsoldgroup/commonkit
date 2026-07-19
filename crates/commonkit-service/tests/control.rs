use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use commonkit_adapters::{
    ExactProviderVersion, MaterializedState, ProviderCapability, ProviderCapabilityResource,
    ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{OperationKind, PlanBindings, ResourceRef, Risk, Sha256Digest, StableId};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::PlanStore;
use commonkit_relay::{
    RelayAdapter, RelayConfig, RelayMutationInputs, RelayPlanRequest, plan_relay_operation,
};
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, DomainFailure, EventHub, ExecutionResult,
    PlanExecutionAuthority, PlanExecutor, RelayProviderAuthority, ServiceStatus, SyncDomain,
    resolved_mcp_from_materialized, router_with_control,
};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", character.to_string().repeat(64))).expect("digest")
}

fn bindings() -> PlanBindings {
    PlanBindings {
        target_identity_digest: digest('3'),
        composed_loadout_digest: digest('4'),
        provider_inputs_digest: digest('5'),
        ownership_map_digest: digest('6'),
        artifact_set_digest: digest('7'),
    }
}

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn plan() -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("files").expect("adapter"),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("file").expect("type"),
            resource_id: StableId::parse("config").expect("resource"),
            managed_path: Some("config.json".into()),
        },
        risk: Risk::Low,
        requires_confirmation: true,
        depends_on: vec![],
        before_digest: None,
        after_digest: Some(digest('d')),
        payload_digest: digest('e'),
        summary: "create config".into(),
    })
    .expect("operation");
    build_plan(PlanDraft {
        target_id: StableId::parse("laptop").expect("target"),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        bindings: bindings(),
        operations: vec![operation],
    })
    .expect("plan")
}

struct SuccessfulExecutor {
    calls: AtomicUsize,
}

impl PlanExecutor for SuccessfulExecutor {
    fn execute(
        &self,
        _plan: &commonkit_contracts::Plan,
        _confirmation_id: &StableId,
    ) -> ExecutionResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ExecutionResult {
            status: ApplyStatus::Succeeded,
            failure_code: None,
        }
    }
}

struct MutableRelayDomain(Mutex<RelayProviderAuthority>);

impl SyncDomain for MutableRelayDomain {
    fn plan(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn relay_provider_authority(&self) -> Result<RelayProviderAuthority, DomainFailure> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn plan_execution_authority(
        &self,
        _: &commonkit_contracts::Plan,
    ) -> Result<PlanExecutionAuthority, DomainFailure> {
        let authority = self.0.lock().unwrap();
        Ok(PlanExecutionAuthority {
            policy_digest: authority.policy_digest.clone(),
            bindings: PlanBindings {
                target_identity_digest: authority.target_identity_digest.clone(),
                composed_loadout_digest: authority.composed_loadout_digest.clone(),
                provider_inputs_digest: authority.provider_inputs_digest.clone(),
                ownership_map_digest: authority.ownership_map_digest.clone(),
                artifact_set_digest: authority.artifact_set_digest.clone(),
            },
        })
    }
}

struct MutablePlanDomain(Mutex<PlanExecutionAuthority>);

impl SyncDomain for MutablePlanDomain {
    fn plan(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
        Err(DomainFailure::OperationFailed)
    }
    fn plan_execution_authority(
        &self,
        _: &commonkit_contracts::Plan,
    ) -> Result<PlanExecutionAuthority, DomainFailure> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn filesystem_apply_recomputes_provider_authority_and_rejects_stale_plan_before_mutation() {
    let root = temporary_directory("filesystem-stale-plan");
    fs::create_dir_all(&root).unwrap();
    let managed = root.join("config.json");
    fs::write(&managed, b"reviewed-bytes").unwrap();
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let control = ControlPlane::new(executor.clone());
    let reviewed = plan();
    let domain = Arc::new(MutablePlanDomain(Mutex::new(PlanExecutionAuthority {
        policy_digest: reviewed.policy_digest.clone(),
        bindings: reviewed.bindings.clone(),
    })));
    control
        .set_target_sync_domains(BTreeMap::from([(
            reviewed.target_id.clone(),
            domain.clone() as Arc<dyn SyncDomain>,
        )]))
        .unwrap();
    let reviewed = control.register_plan(reviewed).unwrap();

    domain.0.lock().unwrap().bindings.provider_inputs_digest = digest('9');
    let token = ControlToken::generate();
    let application = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );
    let response = application
        .oneshot(
            Request::post(format!("/control/v1/plans/{}/apply", reviewed.id))
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "filesystem-apply")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"filesystem-review"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],
        "stale_plan"
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read(&managed).unwrap(), b"reviewed-bytes");
    fs::remove_dir_all(root).unwrap();
}

fn relay_authority() -> RelayProviderAuthority {
    let inputs = ProviderInputs::new(
        StableId::parse("apm").unwrap(),
        ExactProviderVersion::parse("0.25.0").unwrap(),
        "apm.v1".into(),
        BTreeMap::from([("manifest".into(), digest('1'))]),
        vec!["agent-context".into()],
    )
    .unwrap();
    let state = MaterializedState::finalize_with_capabilities(
        inputs.clone(),
        vec![],
        vec![],
        vec![],
        vec![ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: "docs".into(),
                name: "Docs".into(),
                enabled: true,
                url: "https://docs.example/mcp".into(),
                headers: BTreeMap::new(),
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id,
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest,
                source: "apm:mcp:docs".into(),
            },
        }],
    )
    .unwrap();
    RelayProviderAuthority {
        materialized_states: vec![state],
        relay_is_target_local: true,
        target_identity_digest: digest('3'),
        composed_loadout_digest: digest('4'),
        provider_inputs_digest: digest('5'),
        policy_digest: digest('c'),
        ownership_map_digest: digest('6'),
        artifact_set_digest: digest('7'),
    }
}

#[test]
fn relay_apply_recomputes_authority_and_rejects_change_after_review() {
    let root = temporary_directory("relay-authority-change");
    fs::create_dir_all(&root).unwrap();
    let live = root.join("relay.json");
    let state_root = root.join("relay-state");
    let authority = relay_authority();
    let declaration_digest = commonkit_contracts::digest_domain_json(
        "commonkit.resolved-mcp-declarations.v1",
        &resolved_mcp_from_materialized(&authority.materialized_states).unwrap(),
    )
    .unwrap();
    let confirmation = StableId::parse("relay-review").unwrap();
    let mut adapter =
        RelayAdapter::open(StableId::parse("relay").unwrap(), &live, &state_root).unwrap();
    let operation = plan_relay_operation(
        &mut adapter,
        RelayPlanRequest {
            desired: RelayConfig::normalize(serde_json::json!({"servers":[]})).unwrap(),
            inputs: RelayMutationInputs {
                provider_inputs_digest: authority.provider_inputs_digest.clone(),
                policy_digest: authority.policy_digest.clone(),
                target_digest: authority.target_identity_digest.clone(),
                declaration_digest,
                ownership_map_digest: authority.ownership_map_digest.clone(),
                artifact_set_digest: authority.artifact_set_digest.clone(),
                approved_confirmation_id: confirmation.clone(),
                approval_idempotency_key: "relay-apply".into(),
            },
        },
    )
    .unwrap()
    .unwrap();
    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local").unwrap(),
        desired_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: authority.policy_digest.clone(),
        bindings: PlanBindings {
            target_identity_digest: authority.target_identity_digest.clone(),
            composed_loadout_digest: authority.composed_loadout_digest.clone(),
            provider_inputs_digest: authority.provider_inputs_digest.clone(),
            ownership_map_digest: authority.ownership_map_digest.clone(),
            artifact_set_digest: authority.artifact_set_digest.clone(),
        },
        operations: vec![operation],
    })
    .unwrap();
    let domain = Arc::new(MutableRelayDomain(Mutex::new(authority)));
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let control = ControlPlane::new(executor.clone());
    control
        .set_target_sync_domains(BTreeMap::from([(
            StableId::parse("local").unwrap(),
            domain.clone() as Arc<dyn SyncDomain>,
        )]))
        .unwrap();
    control.set_relay_execution_paths(live, state_root);
    let plan = control.register_plan(plan).unwrap();

    domain.0.lock().unwrap().policy_digest = digest('9');
    assert!(
        control
            .apply(&plan.id, &confirmation, "relay-apply")
            .is_err()
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn validates_registered_plans_and_executes_each_idempotency_key_once() {
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let control = ControlPlane::new(executor.clone());
    let plan = control.register_plan(plan()).expect("register");
    let confirmation = StableId::parse("user-approved").expect("confirmation");

    let (first, created) = control
        .apply(&plan.id, &confirmation, "request-001")
        .expect("apply");
    let (second, repeated) = control
        .apply(&plan.id, &confirmation, "request-001")
        .expect("repeat");
    assert!(created);
    assert!(!repeated);
    assert_eq!(first, second);
    assert_eq!(first.status, ApplyStatus::Succeeded);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);

    let replayed_confirmation = StableId::parse("different-review").unwrap();
    assert!(
        control
            .apply(&plan.id, &replayed_confirmation, "request-001")
            .is_err(),
        "an idempotency key cannot replay a different confirmation authority"
    );

    let mut tampered = plan;
    tampered.target_id = StableId::parse("other").expect("target");
    assert!(control.register_plan(tampered).is_err());
}

#[test]
fn reloads_a_registered_plan_after_the_control_plane_restarts() {
    let root = temporary_directory("control-plan-restart");
    let store = Arc::new(PlanStore::open(&root).expect("plan store"));
    let executor = Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    });
    let plan = ControlPlane::with_plan_store(executor.clone(), store.clone())
        .register_plan(plan())
        .expect("register");

    let restarted = ControlPlane::with_plan_store(executor, store);
    assert_eq!(restarted.plan(&plan.id), Some(plan));
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn apply_endpoint_requires_explicit_confirmation_and_idempotency() {
    let token = ControlToken::generate();
    let control = ControlPlane::new(Arc::new(SuccessfulExecutor {
        calls: AtomicUsize::new(0),
    }));
    let plan = control.register_plan(plan()).expect("register");
    let application = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );
    let uri = format!("/control/v1/plans/{}/apply", plan.id);

    let unconfirmed = application
        .clone()
        .oneshot(
            Request::post(&uri)
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "request-002")
                .body(Body::from(
                    r#"{"confirmed":false,"confirmationId":"user-approved"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unconfirmed.status(), StatusCode::CONFLICT);

    let confirmed = application
        .oneshot(
            Request::post(&uri)
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "request-002")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"user-approved"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(confirmed.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn relay_endpoint_rejects_caller_supplied_resolved_declarations() {
    let token = ControlToken::generate();
    let application = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        ControlPlane::new(Arc::new(SuccessfulExecutor {
            calls: AtomicUsize::new(0),
        })),
    );
    let response = application
        .oneshot(
            Request::post("/control/v1/relay/reconcile")
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"targetId":"local","confirmed":false,"confirmationId":"relay-review","idempotencyKey":"relay-1","review":null,"resolved":{"contractVersion":"commonkit.resolved-mcp.v1","declarations":[]}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

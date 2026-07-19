use std::fs;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{PlanBindings, Sha256Digest, StableId, digest_domain_json};
use commonkit_core::{PlanDraft, build_plan};
use commonkit_reconcile::PlanStore;
use commonkit_service::ProductionDomainRegistry;
use commonkit_service::{TargetInventory, TargetRecord, TargetTransport};
use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use commonkit_service::{
    ApplyStatus, ControlPlane, ControlToken, DomainFailure, DriftChecker, EventHub,
    ExecutionResult, OverallState, PlanExecutor, SelectedTargetsDriftChecker, ServiceStatus,
    SyncDomain, router_with_control,
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
    fn git_sync(&self, fetch: bool) -> Result<serde_json::Value, DomainFailure> {
        Ok(serde_json::json!({"targetId":self.0,"state":"behind","fetched":fetch}))
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
    let digest = |value: &str| digest_domain_json("test.target-plan", &value).unwrap();
    let local_plan = build_plan(PlanDraft {
        target_id: StableId::parse("local").unwrap(),
        desired_digest: digest("desired"),
        observed_digest: digest("observed"),
        policy_digest: digest("policy"),
        bindings: PlanBindings {
            target_identity_digest: digest("identity"),
            composed_loadout_digest: digest("loadout"),
            provider_inputs_digest: digest("providers"),
            ownership_map_digest: digest("ownership"),
            artifact_set_digest: digest("artifacts"),
        },
        operations: vec![],
    })
    .unwrap();
    control.register_plan(local_plan.clone()).unwrap();
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
        .clone()
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

    for (method, fetched) in [("GET", false), ("POST", true)] {
        let response = app.clone().oneshot(
            Request::builder().method(method).uri("/control/v1/targets/remote/git")
                .header(header::HOST, "127.0.0.1:3764")
                .header(header::AUTHORIZATION, format!("Bearer {}", token.expose_for_client()))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}")).unwrap()
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["state"], "behind");
        assert_eq!(body["fetched"], fetched);
    }

    let mismatch = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/control/v1/targets/remote/plans/{}/apply",
                    local_plan.id
                ))
                .header(header::HOST, "127.0.0.1:3764")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", token.expose_for_client()),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "mismatch-test")
                .body(Body::from(
                    r#"{"confirmed":true,"confirmationId":"target-mismatch"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatch.status(), StatusCode::CONFLICT);
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

#[test]
fn production_config_builds_a_sync_domain_for_each_local_and_ssh_target() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let digest = |name: &str| digest_domain_json("test.multi-target", &name).unwrap();
    let sync = |id: &str, transport: serde_json::Value| {
        let remote = transport.get("type").and_then(serde_json::Value::as_str) == Some("ssh");
        serde_json::json!({
            "targetId": id,
            "targetRoot": root.join(format!("target-{id}")),
            "adapterState": root.join(format!("adapter-{id}")),
            "providerArtifacts": root.join("artifacts"),
            "materializedStates": [root.join(format!("{id}.state.json"))],
            "targetTransport": transport,
            "targetPlatform": remote.then(|| serde_json::json!({
                "operatingSystem":"linux", "architecture":"x86_64"
            })),
            "declaredRoots": ["home"],
            "protectedRoots": [".commonkit"],
            "caseSensitive": true,
            "targetIdentityDigest": digest(&format!("identity-{id}")),
            "composedLoadoutDigest": digest("loadout"),
            "policyDigest": digest("policy")
        })
    };
    let config = root.join("headless.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "syncTargets": [
                sync("local", serde_json::json!({"type":"local"})),
                sync("remote", serde_json::json!({
                    "type":"ssh", "rootId":"home-root", "host":"remote.internal", "user":"al",
                    "port":22, "knownHosts":root.join("known_hosts"), "fingerprint":"SHA256:test"
                }))
            ]
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
    assert_eq!(registry.target_sync_domains.len(), 2);
    assert_eq!(registry.targets.as_ref().unwrap().targets().len(), 2);
}

#[test]
fn production_config_rejects_ssh_provider_targets_without_platform_facts() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let digest = |name: &str| digest_domain_json("test.remote-platform", &name).unwrap();
    let config = root.join("headless.json");
    fs::write(root.join("known_hosts"), "fixture").unwrap();
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"sync":{
            "targetId":"remote", "targetRoot":root.join("target"),
            "adapterState":root.join("adapter"), "providerArtifacts":root.join("artifacts"),
            "materializedStates":[root.join("state.json")], "targetTransport":{
                "type":"ssh", "rootId":"home-root", "host":"remote.internal", "user":"al",
                "port":22, "knownHosts":root.join("known_hosts"), "fingerprint":"SHA256:test"
            },
            "declaredRoots":["home"], "protectedRoots":[], "caseSensitive":true,
            "targetIdentityDigest":digest("identity"), "composedLoadoutDigest":digest("loadout"),
            "policyDigest":digest("policy")
        }}))
        .unwrap(),
    )
    .unwrap();

    assert!(
        ProductionDomainRegistry::load(
            &config,
            Arc::new(PlanStore::open(root.join("plans")).unwrap()),
            root.join("receipts"),
        )
        .is_err()
    );
}

#[test]
fn production_local_target_executor_applies_to_each_configured_root_and_survives_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let state_path = |id: &str, content: &[u8]| {
        let reference = artifacts
            .put(content, ContentSensitivity::Portable)
            .unwrap();
        let inputs = ProviderInputs::new(
            StableId::parse(format!("native-{id}")).unwrap(),
            ExactProviderVersion::parse("1.0.0").unwrap(),
            "1".into(),
            BTreeMap::from([(
                "input".into(),
                digest_domain_json("test.input", &id).unwrap(),
            )]),
            vec!["files".into()],
        )
        .unwrap();
        let materialized = MaterializedState::finalize(
            inputs.clone(),
            vec![NormalizedResource {
                intent: FilesystemIntent::File {
                    path: NormalizedManagedPath::parse("home/managed.txt").unwrap(),
                    content: reference,
                    mode: None,
                    expected_before: None,
                },
                provenance: ResourceProvenance {
                    provider_id: inputs.provider_id.clone(),
                    provider_version: inputs.provider_version.to_string(),
                    input_digest: inputs.input_set_digest,
                    source: format!("fixture-{id}"),
                },
            }],
            vec![],
            vec![],
        )
        .unwrap();
        let path = root.join(format!("{id}.state.json"));
        fs::write(&path, serde_json::to_vec(&materialized).unwrap()).unwrap();
        path
    };
    let first_state = state_path("first", b"first\n");
    let second_state = state_path("second", b"second\n");
    let digest = |value: &str| digest_domain_json("test.multi-executor", &value).unwrap();
    let sync = |id: &str, state: &std::path::Path| {
        serde_json::json!({
            "targetId":id, "targetRoot":root.join(format!("target-{id}")),
            "adapterState":root.join(format!("adapter-{id}")), "providerArtifacts":root.join("artifacts"),
            "materializedStates":[state], "targetTransport":{"type":"local"},
            "declaredRoots":["home"], "protectedRoots":[], "caseSensitive":true,
            "targetIdentityDigest":digest(&format!("identity-{id}")),
            "composedLoadoutDigest":digest("loadout"), "policyDigest":digest("policy")
        })
    };
    let config = root.join("headless.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "syncTargets":[sync("first", &first_state), sync("second", &second_state)]
        }))
        .unwrap(),
    )
    .unwrap();
    let plans = Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let registry =
        ProductionDomainRegistry::load(&config, plans.clone(), root.join("receipts")).unwrap();
    let executors = registry
        .target_executors(plans.clone(), root.join("receipts"))
        .unwrap();
    for id in ["first", "second"] {
        let target = StableId::parse(id).unwrap();
        let plan_value = registry.target_sync_domains[&target]
            .plan(serde_json::json!({"confirmed":true,"confirmationId":"integration-plan"}))
            .unwrap();
        let plan: commonkit_contracts::Plan = serde_json::from_value(plan_value).unwrap();
        assert_eq!(
            executors[&target]
                .execute(&plan, &StableId::parse("integration-apply").unwrap())
                .status,
            ApplyStatus::Succeeded
        );
        assert_eq!(
            fs::read(root.join(format!("target-{id}/home/managed.txt"))).unwrap(),
            format!("{id}\n").as_bytes()
        );
    }
    drop(registry);
    let reopened = ProductionDomainRegistry::load(&config, plans, root.join("receipts")).unwrap();
    assert_eq!(reopened.target_sync_domains.len(), 2);
    for id in ["first", "second"] {
        let verified = reopened.target_sync_domains[&StableId::parse(id).unwrap()]
            .verify(serde_json::json!({}))
            .unwrap();
        assert_eq!(verified["verified"], true);
    }
}

#[test]
fn scheduler_verifies_every_selected_target_read_only_and_reports_partial_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let inventory = Arc::new(
        TargetInventory::open(
            temporary.path().join("targets.json"),
            vec![
                target("healthy", TargetTransport::Local),
                target("drifted", TargetTransport::Local),
            ],
            Some(vec![
                StableId::parse("healthy").unwrap(),
                StableId::parse("drifted").unwrap(),
            ]),
        )
        .unwrap(),
    );
    struct ResultDomain(Result<serde_json::Value, DomainFailure>);
    impl SyncDomain for ResultDomain {
        fn plan(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
            panic!("scheduler must not plan")
        }
        fn verify(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
            self.0.clone()
        }
        fn rollback(&self, _: serde_json::Value) -> Result<serde_json::Value, DomainFailure> {
            panic!("scheduler must not mutate")
        }
    }
    let checker = SelectedTargetsDriftChecker::new(
        inventory,
        BTreeMap::from([
            (
                StableId::parse("healthy").unwrap(),
                Arc::new(ResultDomain(Ok(serde_json::json!({"state":"healthy"}))))
                    as Arc<dyn SyncDomain>,
            ),
            (
                StableId::parse("drifted").unwrap(),
                Arc::new(ResultDomain(Err(DomainFailure::VerificationFailed)))
                    as Arc<dyn SyncDomain>,
            ),
        ]),
    );
    let result = checker.check();
    assert_eq!(result.state, OverallState::Drifted);
    assert_eq!(result.code.as_deref(), Some("selected_target_drifted"));
}

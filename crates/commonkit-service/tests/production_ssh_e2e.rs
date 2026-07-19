use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use commonkit_adapters::*;
use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use commonkit_reconcile::PlanStore;
use commonkit_service::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[derive(Default)]
struct Remote {
    files: BTreeMap<String, Vec<u8>>,
    staged: BTreeMap<Sha256Digest, Vec<u8>>,
}
#[derive(Clone, Default)]
struct Memory(Arc<Mutex<Remote>>);
impl SshFilesystemTransport for Memory {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        let mut remote = self.0.lock().unwrap();
        match request {
            SshFilesystemRequest::ReadFile { path, .. } => Ok(remote
                .files
                .get(path.as_str())
                .cloned()
                .map(|content| SshFilesystemResponse::File { content })
                .unwrap_or(SshFilesystemResponse::Absent)),
            SshFilesystemRequest::WriteFile { path, content, .. } => {
                remote.files.insert(path.to_string(), content);
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::Remove { path, .. } => {
                remote.files.remove(path.as_str());
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::StageArtifact {
                digest, content, ..
            } => {
                remote.staged.insert(digest.clone(), content);
                Ok(SshFilesystemResponse::ArtifactStaged { digest })
            }
            SshFilesystemRequest::VerifyArtifact { digest, .. }
                if remote.staged.contains_key(&digest) =>
            {
                Ok(SshFilesystemResponse::ArtifactVerified { digest })
            }
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }
}
struct Factory(Memory);
impl ProductionSshTransportFactory for Factory {
    fn open(
        &self,
        _: &ProductionSshTarget,
    ) -> Result<Box<dyn SshFilesystemTransport + Send>, DomainFailure> {
        Ok(Box::new(self.0.clone()))
    }
}
struct Fallback;
impl PlanExecutor for Fallback {
    fn execute(&self, _: &commonkit_contracts::Plan, _: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: Some(StableId::parse("wrong_executor").unwrap()),
        }
    }
}
fn digest(value: &str) -> Sha256Digest {
    digest_domain_json("test.production-ssh.v1", &value).unwrap()
}
fn write_json(path: &std::path::Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

async fn call(
    app: axum::Router,
    token: &ControlToken,
    method: &str,
    path: &str,
    body: Value,
    idempotency: Option<&str>,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "127.0.0.1:3764")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token.expose_for_client()),
        )
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = idempotency {
        request = request.header("idempotency-key", key);
    }
    let response = app
        .oneshot(
            request
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    assert!(
        status.is_success(),
        "{method} {path}: {status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

fn setup(
    root: &std::path::Path,
    remote: Memory,
) -> (
    ProductionDomainRegistry,
    Arc<PlanStore>,
    Arc<dyn PlanExecutor>,
) {
    setup_with_capabilities(root, remote, false)
}

fn setup_with_capabilities(
    root: &std::path::Path,
    remote: Memory,
    with_mcp: bool,
) -> (
    ProductionDomainRegistry,
    Arc<PlanStore>,
    Arc<dyn PlanExecutor>,
) {
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "native.v1".into(),
        BTreeMap::from([("git".into(), digest("revision"))]),
        vec!["files".into()],
    )
    .unwrap();
    let capabilities = with_mcp.then(|| ProviderCapabilityResource {
        capability: ProviderCapability::McpStreamableHttp {
            id: "docs".into(),
            name: "Docs".into(),
            enabled: true,
            url: "https://docs.example/mcp".into(),
            headers: BTreeMap::new(),
        },
        provenance: ResourceProvenance {
            provider_id: inputs.provider_id.clone(),
            provider_version: inputs.provider_version.to_string(),
            input_digest: inputs.input_set_digest.clone(),
            source: "native:mcp:docs".into(),
        },
    });
    let state = MaterializedState::finalize_with_capabilities(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/editor.conf").unwrap(),
                content: artifacts
                    .put(b"managed\n", ContentSensitivity::Portable)
                    .unwrap(),
                mode: None,
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "portable/editor.conf".into(),
            },
        }],
        vec![],
        vec![],
        capabilities.into_iter().collect(),
    )
    .unwrap();
    let state_path = root.join("state.json");
    write_json(&state_path, &state);
    std::fs::write(root.join("known_hosts"), "fixture").unwrap();
    std::fs::create_dir_all(root.join("controller-target")).unwrap();
    let config = serde_json::json!({"sync":{
        "targetId":"remote-linux", "targetRoot":root.join("controller-target"), "adapterState":root.join("adapter"),
        "providerArtifacts":root.join("artifacts"), "materializedStates":[state_path],
        "targetTransport":{"type":"ssh","rootId":"home-root","host":"fixture","user":"al","port":22,
            "knownHosts":root.join("known_hosts"),"fingerprint":"SHA256:fixturefixturefixture"},
        "targetPlatform":{"operatingSystem":"linux","architecture":"x86_64"},
        "declaredRoots":["home"],"relayClientRoot":"home","protectedRoots":[],"caseSensitive":true,
        "targetIdentityDigest":digest("target"),"composedLoadoutDigest":digest("loadout"),"policyDigest":digest("policy")
    }});
    let config_path = root.join("headless.json");
    write_json(&config_path, &config);
    let plans = Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let factory = Arc::new(Factory(remote));
    let registry = ProductionDomainRegistry::load_with_ssh_factory(
        &config_path,
        plans.clone(),
        root.join("receipts"),
        factory.clone(),
    )
    .unwrap();
    let ssh = registry
        .ssh_executor_with_factory(plans.clone(), root.join("receipts"), factory)
        .unwrap()
        .unwrap();
    let dispatch: Arc<dyn PlanExecutor> = Arc::new(TargetDispatchPlanExecutor::new(
        Arc::new(Fallback),
        Some(ssh),
    ));
    (registry, plans, dispatch)
}

#[test]
fn ssh_target_with_provider_mcp_requires_a_target_resident_daemon() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (registry, _, _) = setup_with_capabilities(temporary.path(), remote, true);
    assert_eq!(
        registry
            .sync
            .as_ref()
            .unwrap()
            .plan(serde_json::json!({
                "confirmed": true,
                "confirmationId": "relay-plan"
            })),
        Err(DomainFailure::RelayRequiresTargetResidentDaemon)
    );
}

async fn apply_and_wait(
    app: axum::Router,
    token: &ControlToken,
    plan: &Value,
    confirmation: &str,
    key: &str,
) -> Value {
    call(
        app.clone(),
        token,
        "POST",
        "/control/v1/plans",
        plan.clone(),
        None,
    )
    .await;
    let operation = call(
        app.clone(),
        token,
        "POST",
        &format!("/control/v1/plans/{}/apply", plan["id"].as_str().unwrap()),
        serde_json::json!({"confirmed":true,"confirmationId":confirmation}),
        Some(key),
    )
    .await;
    for _ in 0..100 {
        let current = call(
            app.clone(),
            token,
            "GET",
            &format!(
                "/control/v1/operations/{}",
                operation["id"].as_str().unwrap()
            ),
            serde_json::json!({}),
            None,
        )
        .await;
        if current["status"] != "running" {
            return current;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("apply did not finish")
}

#[tokio::test]
async fn authenticated_sync_plan_dispatches_ssh_apply_rejects_stale_and_survives_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (registry, plans, executor) = setup(temporary.path(), remote.clone());
    let token = ControlToken::generate();
    let control = ControlPlane::with_plan_store(executor, plans.clone());
    control.set_headless_domains(registry.into_headless());
    let app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        control,
    );

    let stale = call(
        app.clone(),
        &token,
        "POST",
        "/control/v1/sync/plan",
        serde_json::json!({"confirmed":true,"confirmationId":"plan-stale"}),
        None,
    )
    .await;
    remote
        .0
        .lock()
        .unwrap()
        .files
        .insert("home/editor.conf".into(), b"hand edit".to_vec());
    assert_eq!(
        apply_and_wait(app.clone(), &token, &stale, "apply-stale", "stale-key").await["status"],
        "rolled_back"
    );
    remote.0.lock().unwrap().files.clear();
    let plan = call(
        app.clone(),
        &token,
        "POST",
        "/control/v1/sync/plan",
        serde_json::json!({"confirmed":true,"confirmationId":"plan-ok"}),
        None,
    )
    .await;
    assert_eq!(
        apply_and_wait(app.clone(), &token, &plan, "apply-ok", "ok-key").await["status"],
        "succeeded"
    );
    assert_eq!(
        remote.0.lock().unwrap().files["home/editor.conf"],
        b"managed\n"
    );
    assert_eq!(
        call(
            app.clone(),
            &token,
            "POST",
            "/control/v1/verify",
            serde_json::json!({"planId":plan["id"]}),
            None
        )
        .await["verified"],
        true
    );

    let (registry, _, executor) = setup(temporary.path(), remote.clone());
    let restarted = ControlPlane::with_plan_store(executor, plans);
    restarted.set_headless_domains(registry.into_headless());
    let restarted_app = router_with_control(
        token.clone(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        "127.0.0.1:3764",
        EventHub::new(8),
        restarted,
    );
    assert_eq!(
        apply_and_wait(restarted_app, &token, &plan, "apply-ok", "restart-key").await["status"],
        "succeeded"
    );
}

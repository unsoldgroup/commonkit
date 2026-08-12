use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use commonkit_adapters::*;
use commonkit_contracts::{
    OperationKind, PackageConsent, PackageDeclaration, PackageManager,
    PackageOperationConsentBinding, PackageSelector, PlanBindings, RecoveryCapability, ResourceRef,
    Risk, SchemaVersion, SecurityPolicy, Sha256Digest, StableId, digest_domain_json,
    package_operation_set_digest,
};
use commonkit_core::{OperationDraft, PlanDraft, build_plan, finalize_operation};
use commonkit_reconcile::{PlanStore, ReceiptJournal, ReceiptStore};
use commonkit_service::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[derive(Default)]
struct Remote {
    files: BTreeMap<String, Vec<u8>>,
    staged: BTreeMap<Sha256Digest, Vec<u8>>,
    opens: usize,
    installed_packages: BTreeSet<String>,
    package_phases: Vec<PackageMutationPhase>,
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
            SshFilesystemRequest::InspectResource { path, .. } => Ok(remote
                .files
                .get(path.as_str())
                .cloned()
                .map(|content| SshFilesystemResponse::Resource {
                    resource: TargetResource::File { content },
                })
                .unwrap_or(SshFilesystemResponse::Resource {
                    resource: TargetResource::Absent,
                })),
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
            SshFilesystemRequest::PackageMutation {
                phase, resolution, ..
            } => {
                remote.package_phases.push(phase);
                match phase {
                    PackageMutationPhase::Observe => Ok(SshFilesystemResponse::PackageObserved {
                        installed_versions: remote.installed_packages.clone(),
                    }),
                    PackageMutationPhase::Apply => {
                        for package in &resolution.closure {
                            let Some(PackageSelector::AptBinary { name, architecture }) =
                                package.declaration.selector.as_ref()
                            else {
                                return Err(TargetFilesystemError::InvalidRemoteResponse);
                            };
                            let architecture =
                                architecture.as_deref().unwrap_or(&resolution.target.arch);
                            remote.installed_packages.insert(format!(
                                "{name}:{architecture}={}",
                                package.declaration.version
                            ));
                        }
                        Ok(SshFilesystemResponse::Applied)
                    }
                    PackageMutationPhase::Prepare | PackageMutationPhase::Verify => {
                        Ok(SshFilesystemResponse::Applied)
                    }
                }
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
        self.0.0.lock().unwrap().opens += 1;
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

fn apt_resolution() -> (PackageResolutionV1, PackageResolutionAuthority) {
    let source_id = StableId::parse("ubuntu-main").unwrap();
    let target = PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "amd64".into(),
        libc: Some("glibc".into()),
        manager_prefix: None,
    };
    let manager = ManagerBindingV1 {
        manager: PackageManager::Apt,
        version: "2.7.14".into(),
        executable_digest: digest("apt-executable"),
        config_digest: digest("apt-config"),
    };
    let registry = PackageSourceRegistry::builtin()
        .unwrap()
        .with_apt_source_authority(
            &source_id,
            AptSourceAuthorityV1 {
                suite: "noble".into(),
                components: BTreeSet::from(["main".into()]),
                signing_authority: StableId::parse("ubuntu-archive").unwrap(),
                signing_key_digest: digest("apt-key"),
            },
        )
        .unwrap();
    let policy = SecurityPolicy {
        allowlists: [(
            StableId::parse("package_sources").unwrap(),
            BTreeSet::from(["ubuntu-main".into()]),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    let authority = PackageResolutionAuthority::new(&target, &manager, &registry, &policy).unwrap();
    let declaration = PackageDeclaration {
        id: StableId::parse("ripgrep").unwrap(),
        version: "14.1.1-1ubuntu1".into(),
        manager: PackageManager::Apt,
        source: source_id.clone(),
        selector: Some(PackageSelector::AptBinary {
            name: "ripgrep".into(),
            architecture: Some("amd64".into()),
        }),
    };
    let source = SourceBindingV1 {
        source_id,
        registry_definition_digest: registry
            .source_definition_digest(&StableId::parse("ubuntu-main").unwrap())
            .unwrap(),
        canonical_repository: "https://archive.ubuntu.com/ubuntu".into(),
        repository_revision: Some(digest("apt-metadata").as_str()[7..].into()),
        signed_metadata: vec![ArtifactEvidence {
            authority: StableId::parse("ubuntu-archive").unwrap(),
            metadata_digest: digest("apt-metadata"),
            signature_digest: digest("apt-key"),
        }],
    };
    (
        PackageResolutionV1 {
            schema_version: SchemaVersion(1),
            declaration: declaration.clone(),
            target,
            manager,
            source: source.clone(),
            before: PackageObservationV1 {
                installed_versions: BTreeSet::new(),
            },
            closure: vec![ResolvedPackage {
                declaration,
                source,
            }],
            artifacts: Vec::new(),
            recipe: OfflineInstallRecipeV1::AptArchives {
                artifact_roles: BTreeSet::new(),
            },
        },
        authority,
    )
}

fn package_plan(
    resolution_digest: Sha256Digest,
    authority_digest: Sha256Digest,
) -> commonkit_contracts::Plan {
    package_plan_with_identity(resolution_digest, authority_digest, digest("target"))
}

fn package_plan_with_identity(
    resolution_digest: Sha256Digest,
    authority_digest: Sha256Digest,
    target_identity_digest: Sha256Digest,
) -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("packages").unwrap(),
        kind: OperationKind::Create,
        resource: ResourceRef {
            resource_type: StableId::parse("package").unwrap(),
            resource_id: StableId::parse("ripgrep").unwrap(),
            managed_path: None,
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        recovery_capability: RecoveryCapability::ConvergeForwardOnly,
        depends_on: Vec::new(),
        before_digest: None,
        after_digest: Some(digest("package-after")),
        payload_digest: resolution_digest,
        provenance: None,
        summary: "install ripgrep".into(),
    })
    .unwrap();
    build_plan(PlanDraft {
        target_id: StableId::parse("remote-linux").unwrap(),
        desired_digest: digest("package-desired"),
        observed_digest: digest("package-observed"),
        policy_digest: digest("package-policy"),
        bindings: PlanBindings {
            target_identity_digest,
            composed_loadout_digest: digest("loadout"),
            provider_inputs_digest: digest("provider-inputs"),
            ownership_map_digest: digest("ownership"),
            artifact_set_digest: digest("artifacts"),
            package_resolution_authority_digest: Some(authority_digest),
        },
        operations: vec![operation],
    })
    .unwrap()
}

fn filesystem_plan() -> commonkit_contracts::Plan {
    let operation = finalize_operation(OperationDraft {
        adapter_id: StableId::parse("files").unwrap(),
        kind: OperationKind::Update,
        resource: ResourceRef {
            resource_type: StableId::parse("file").unwrap(),
            resource_id: StableId::parse("editor-conf").unwrap(),
            managed_path: None,
        },
        risk: Risk::Low,
        requires_confirmation: false,
        recovery_capability: RecoveryCapability::ExactRollback,
        depends_on: Vec::new(),
        before_digest: Some(digest("before")),
        after_digest: Some(digest("after")),
        payload_digest: digest("payload"),
        provenance: None,
        summary: "update editor.conf".into(),
    })
    .unwrap();
    build_plan(PlanDraft {
        target_id: StableId::parse("remote-linux").unwrap(),
        desired_digest: digest("desired"),
        observed_digest: digest("observed"),
        policy_digest: digest("policy"),
        bindings: PlanBindings {
            target_identity_digest: digest("target"),
            composed_loadout_digest: digest("loadout"),
            provider_inputs_digest: digest("provider-inputs"),
            ownership_map_digest: digest("ownership"),
            artifact_set_digest: digest("artifacts"),
            package_resolution_authority_digest: None,
        },
        operations: vec![operation],
    })
    .unwrap()
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
    Arc<dyn PlanExecutor>,
) {
    setup_with_capabilities(root, remote, false, true)
}

fn setup_non_root(
    root: &std::path::Path,
    remote: Memory,
) -> (
    ProductionDomainRegistry,
    Arc<PlanStore>,
    Arc<dyn PlanExecutor>,
    Arc<dyn PlanExecutor>,
) {
    setup_with_capabilities(root, remote, false, false)
}

fn setup_with_capabilities(
    root: &std::path::Path,
    remote: Memory,
    with_mcp: bool,
    root_capable: bool,
) -> (
    ProductionDomainRegistry,
    Arc<PlanStore>,
    Arc<dyn PlanExecutor>,
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
            }
            .into(),
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
        "targetTransport":{"type":"ssh","rootId":"home-root","host":"fixture","user":if root_capable {"root"} else {"al"},"port":22,"rootCapable":root_capable,
            "knownHosts":root.join("known_hosts"),"fingerprint":"SHA256:fixturefixturefixture"},
        "targetPlatform":{"operatingSystem":"linux","architecture":"x86_64"},
        "packageResolution": {
            "target": {"os":"linux", "osVersion":"24.04", "distroId":"ubuntu", "distroVersion":"24.04", "codename":"noble", "arch":"amd64", "libc":"glibc"},
            "manager": {"manager":"apt", "version":"2.7.14", "executableDigest":digest("apt-executable"), "configDigest":digest("apt-config")},
            "policy": serde_json::to_value(SecurityPolicy {
                allowlists: [(
                    StableId::parse("package_sources").unwrap(),
                    BTreeSet::from(["ubuntu-main".into()]),
                )]
                .into_iter()
                .collect(),
                ..SecurityPolicy::default()
            }).unwrap(),
            "apt": {
                "sourceId": "ubuntu-main",
                "suite": "noble",
                "components": ["main"],
                "signedBy": "/usr/share/keyrings/ubuntu-archive-keyring.gpg",
                "signingAuthority": "ubuntu-archive",
                "signingKeyDigest": digest("apt-key"),
                "trustedMetadataDigest": digest("apt-metadata")
            }
        },
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
        Some(ssh.clone()),
    ));
    (registry, plans, dispatch, ssh)
}

#[test]
fn ssh_target_with_provider_mcp_requires_a_target_resident_daemon() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (registry, _, _, _) = setup_with_capabilities(temporary.path(), remote, true, true);
    assert_eq!(
        registry.sync.as_ref().unwrap().plan(serde_json::json!({
            "confirmed": true,
            "confirmationId": "relay-plan"
        })),
        Err(DomainFailure::RelayRequiresTargetResidentDaemon)
    );
}

#[test]
fn approved_package_plan_opens_the_typed_ssh_adapter_instead_of_the_unavailable_fallback() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (_, plans, _, executor) = setup(temporary.path(), remote.clone());
    let (resolution, _authority) = apt_resolution();
    let package_artifacts = ArtifactStore::open(temporary.path().join("adapter/packages")).unwrap();
    let resolution_ref = package_artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let package_policy = SecurityPolicy {
        allowlists: [(
            StableId::parse("package_sources").unwrap(),
            BTreeSet::from(["ubuntu-main".into()]),
        )]
        .into_iter()
        .collect(),
        ..SecurityPolicy::default()
    };
    let (remote_authority, _, _) =
        PackageResolutionAuthority::load_remote_by_resolution_digest(
            &resolution_ref.digest,
            &package_artifacts,
            &package_policy,
        )
        .unwrap();
    let plan = package_plan(resolution_ref.digest, remote_authority.digest().clone());
    plans.persist(&plan).unwrap();
    let confirmation_id = StableId::parse("package-confirmation").unwrap();
    let consent = PackageConsent {
        confirmation_id: confirmation_id.clone(),
        operation_set_digest: package_operation_set_digest(
            &plan,
            &[PackageOperationConsentBinding {
                operation_id: plan.operations[0].id.clone(),
                resolution_digest: plan.operations[0].payload_digest.clone(),
            }],
        )
        .unwrap(),
    };

    let result = executor.execute_with_package_consent(&plan, &confirmation_id, &consent);
    assert_eq!(result.status, ApplyStatus::Succeeded);
    assert_eq!(result.failure_code, None);
    let remote = remote.0.lock().unwrap();
    assert!(remote.opens >= 2);
    assert_eq!(
        remote.package_phases,
        vec![
            PackageMutationPhase::Prepare,
            PackageMutationPhase::Apply,
            PackageMutationPhase::Verify,
            PackageMutationPhase::Observe,
        ]
    );
}

#[test]
fn non_root_ssh_apt_rejects_before_receipt_or_transport() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (_, plans, _, executor) = setup_non_root(temporary.path(), remote.clone());
    let (resolution, authority) = apt_resolution();
    let package_artifacts = ArtifactStore::open(temporary.path().join("adapter/packages")).unwrap();
    let resolution_ref = package_artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let plan = package_plan(resolution_ref.digest, authority.digest().clone());
    plans.persist(&plan).unwrap();
    let confirmation_id = StableId::parse("package-confirmation").unwrap();
    let consent = PackageConsent {
        confirmation_id: confirmation_id.clone(),
        operation_set_digest: package_operation_set_digest(
            &plan,
            &[PackageOperationConsentBinding {
                operation_id: plan.operations[0].id.clone(),
                resolution_digest: plan.operations[0].payload_digest.clone(),
            }],
        )
        .unwrap(),
    };

    let result = executor.execute_with_package_consent(&plan, &confirmation_id, &consent);

    assert_eq!(result.status, ApplyStatus::Failed);
    assert_eq!(remote.0.lock().unwrap().opens, 0);
    assert!(
        std::fs::read_dir(temporary.path().join("receipts"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn ssh_executor_rejects_rotated_target_identity_before_transport() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (_, plans, _, executor) = setup(temporary.path(), remote.clone());
    let (resolution, authority) = apt_resolution();
    let package_artifacts = ArtifactStore::open(temporary.path().join("adapter/packages")).unwrap();
    let resolution_ref = package_artifacts
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let plan = package_plan_with_identity(
        resolution_ref.digest,
        authority.digest().clone(),
        digest("rotated-target"),
    );
    plans.persist(&plan).unwrap();
    let confirmation_id = StableId::parse("package-confirmation").unwrap();
    let consent = PackageConsent {
        confirmation_id: confirmation_id.clone(),
        operation_set_digest: package_operation_set_digest(
            &plan,
            &[PackageOperationConsentBinding {
                operation_id: plan.operations[0].id.clone(),
                resolution_digest: plan.operations[0].payload_digest.clone(),
            }],
        )
        .unwrap(),
    };

    let result = executor.execute_with_package_consent(&plan, &confirmation_id, &consent);

    assert_eq!(result.status, ApplyStatus::Failed);
    assert_eq!(remote.0.lock().unwrap().opens, 0);
}

#[test]
fn ssh_executor_rejects_tampered_receipt_before_transport() {
    let temporary = tempfile::tempdir().unwrap();
    let remote = Memory::default();
    let (_, plans, _, executor) = setup(temporary.path(), remote.clone());
    let plan = filesystem_plan();
    plans.persist(&plan).unwrap();
    let confirmation = StableId::parse("tampered-receipt-confirmation").unwrap();
    let run_digest = digest_domain_json(
        "commonkit.production-ssh-run.v1",
        &(&plan.id, &confirmation),
    )
    .unwrap();
    let run_id = StableId::parse(format!("run-{}", &run_digest.as_str()[7..55])).unwrap();
    let receipts = ReceiptStore::open(temporary.path().join("receipts")).unwrap();
    receipts
        .persist(&ReceiptJournal::for_plan(run_id.clone(), &plan).unwrap())
        .unwrap();
    let receipt_path = std::fs::read_dir(temporary.path().join("receipts").join(run_id.as_str()))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(receipt_path, b"{}").unwrap();

    let result = executor.execute(&plan, &confirmation);

    assert_eq!(result.status, ApplyStatus::Failed);
    assert_eq!(remote.0.lock().unwrap().opens, 0);
}

async fn apply_and_wait(
    app: axum::Router,
    token: &ControlToken,
    plan: &Value,
    confirmation: &str,
    key: &str,
) -> Value {
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
    let (registry, plans, executor, _) = setup(temporary.path(), remote.clone());
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

    let (registry, _, executor, _) = setup(temporary.path(), remote.clone());
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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use commonkit_adapters::{
    ArtifactEvidence, ArtifactStore, ContentSensitivity, DeclaredSideEffect, ExactProviderVersion,
    FileAdapter, FilesystemIntent, ManagerBindingV1, MaterializedState, NormalizedManagedPath,
    NormalizedResource, OfflineInstallRecipeV1, OwnershipRules, PackageArtifactV1,
    PackageObservationV1, PackageResolutionV1, PackageResourcePlanner, PackageTargetV1,
    ProviderInputs, ProviderPlanError, ProviderPlanRequest, ProviderPlannerRoute,
    ProviderResourceRouter, ResolvedMaterializedState, ResolvedPackage, ResolvedPackageIntent,
    ResourceProvenance, ResourceType, SourceBindingV1, UnsupportedCapability, build_provider_plan,
    build_resolved_provider_plan_with_router,
};
use commonkit_contracts::{
    Operation, OperationKind, PackageDeclaration, PackageManager, RecoveryCapability, ResourceRef,
    Risk, SchemaVersion, Sha256Digest, StableId,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::Adapter;
use sha2::{Digest, Sha256};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn state(
    provider: &str,
    version: &str,
    input: &str,
    path: &str,
    content: commonkit_adapters::ContentReference,
) -> MaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse(provider).unwrap(),
        ExactProviderVersion::parse(version).unwrap(),
        "provider.v1".into(),
        BTreeMap::from([("source".into(), bytes_digest(input.as_bytes()))]),
        vec!["filesystem".into()],
    )
    .unwrap();
    MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(path).unwrap(),
                content,
                mode: None,
                expected_before: None,
            }
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: version.into(),
                input_digest: inputs.digest().clone(),
                source: format!("fixture:{path}"),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap()
}

fn bytes_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes))).unwrap()
}

fn mixed_state(artifacts: &ArtifactStore) -> ResolvedMaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "provider.v2".into(),
        BTreeMap::from([("source".into(), digest('a'))]),
        vec!["filesystem".into(), "package".into()],
    )
    .unwrap();
    let file_content = artifacts
        .put(b"config", ContentSensitivity::Portable)
        .unwrap();
    let package_artifact = artifacts
        .put(b"package", ContentSensitivity::Portable)
        .unwrap();
    let declaration = PackageDeclaration {
        id: StableId::parse("ripgrep").unwrap(),
        version: "14.1.1".into(),
        manager: PackageManager::Homebrew,
        source: StableId::parse("homebrew-core").unwrap(),
        selector: None,
    };
    let source = SourceBindingV1 {
        source_id: declaration.source.clone(),
        registry_definition_digest: digest('1'),
        canonical_repository: "https://github.com/Homebrew/homebrew-core".into(),
        repository_revision: Some("0123456789abcdef0123456789abcdef01234567".into()),
        signed_metadata: vec![ArtifactEvidence {
            authority: StableId::parse("homebrew").unwrap(),
            metadata_digest: digest('2'),
            signature_digest: digest('3'),
        }],
    };
    let source_metadata_digest = source.metadata_digest().unwrap();
    let package_resolution = PackageResolutionV1 {
        schema_version: SchemaVersion(1),
        declaration: declaration.clone(),
        target: PackageTargetV1 {
            os: "macos".into(),
            os_version: "15.6".into(),
            distro_id: None,
            distro_version: None,
            codename: None,
            arch: "aarch64".into(),
            libc: None,
            manager_prefix: Some("/opt/homebrew".into()),
        },
        manager: ManagerBindingV1 {
            manager: PackageManager::Homebrew,
            version: "4.5.0".into(),
            executable_digest: digest('e'),
            config_digest: digest('f'),
        },
        source,
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        closure: vec![ResolvedPackage {
            declaration: declaration.clone(),
        }],
        artifacts: vec![PackageArtifactV1 {
            role: StableId::parse("bottle").unwrap(),
            content: package_artifact.clone(),
            upstream_checksum: package_artifact.digest.clone(),
            size: package_artifact.bytes,
            materialization_key: StableId::parse("ripgrep-bottle").unwrap(),
            source_metadata_digest,
        }],
        recipe: OfflineInstallRecipeV1::HomebrewBottle {
            artifact_roles: BTreeSet::from([StableId::parse("bottle").unwrap()]),
        },
    };
    let resolution = artifacts
        .put(
            &serde_json::to_vec(&package_resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    let provenance = |source: &str| ResourceProvenance {
        provider_id: inputs.provider_id.clone(),
        provider_version: inputs.provider_version.to_string(),
        input_digest: inputs.digest().clone(),
        source: source.into(),
    };
    ResolvedMaterializedState::finalize(
        inputs.clone(),
        vec![
            NormalizedResource {
                intent: FilesystemIntent::File {
                    path: NormalizedManagedPath::parse("home/config").unwrap(),
                    content: file_content,
                    mode: None,
                    expected_before: None,
                }
                .into(),
                provenance: provenance("file"),
            },
            NormalizedResource {
                intent: ResolvedPackageIntent {
                    declaration,
                    resolution,
                    artifacts: vec![package_artifact],
                }
                .into(),
                provenance: provenance("package"),
            },
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap()
}

struct NoopPackagePlanner {
    id: StableId,
}

struct SpoofPackagePlanner {
    declared: StableId,
    returned: StableId,
    spoof_resource_binding: bool,
}

impl PackageResourcePlanner for SpoofPackagePlanner {
    fn adapter_id(&self) -> &StableId {
        &self.declared
    }

    fn register_package_resource(
        &mut self,
        id: StableId,
        intent: &ResolvedPackageIntent,
        provenance: &ResourceProvenance,
        _provider_artifacts: &ArtifactStore,
    ) -> Result<Option<Operation>, ProviderPlanError> {
        let desired_digest =
            commonkit_adapters::ResourceIntent::ResolvedPackage(intent.clone()).desired_digest()?;
        let resource = if self.spoof_resource_binding {
            ResourceRef {
                resource_type: StableId::parse("service").unwrap(),
                resource_id: StableId::parse("provider-chosen-resource").unwrap(),
                managed_path: Some("home/provider-chosen".into()),
            }
        } else {
            ResourceRef {
                resource_type: StableId::parse("package").unwrap(),
                resource_id: id,
                managed_path: None,
            }
        };
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.returned.clone(),
            kind: OperationKind::Create,
            resource,
            risk: Risk::Medium,
            requires_confirmation: true,
            recovery_capability: if self.spoof_resource_binding {
                RecoveryCapability::ExactRollback
            } else {
                RecoveryCapability::ConvergeForwardOnly
            },
            depends_on: vec![],
            before_digest: None,
            after_digest: Some(if self.spoof_resource_binding {
                digest('e')
            } else {
                desired_digest.clone()
            }),
            payload_digest: if self.spoof_resource_binding {
                digest('f')
            } else {
                desired_digest
            },
            provenance: (!self.spoof_resource_binding).then(|| provenance.clone()),
            summary: "install package".into(),
        })?;
        Ok(Some(operation))
    }
}

impl NoopPackagePlanner {
    fn new(id: &str) -> Self {
        Self {
            id: StableId::parse(id).unwrap(),
        }
    }
}

impl PackageResourcePlanner for NoopPackagePlanner {
    fn adapter_id(&self) -> &StableId {
        &self.id
    }

    fn register_package_resource(
        &mut self,
        _id: StableId,
        _intent: &ResolvedPackageIntent,
        _provenance: &ResourceProvenance,
        _provider_artifacts: &ArtifactStore,
    ) -> Result<Option<Operation>, ProviderPlanError> {
        Ok(None)
    }
}

#[test]
fn missing_package_route_fails_before_filesystem_operation_registration() {
    let root = temporary_directory("provider-plan-missing-package-route");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let state = mixed_state(&artifacts);
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let adapter_state = root.join("adapter-state");
    let mut files = FileAdapter::open(&root.join("target"), &adapter_state).unwrap();
    let mut router =
        ProviderResourceRouter::new(vec![ProviderPlannerRoute::Filesystem(&mut files)]).unwrap();
    let error = build_resolved_provider_plan_with_router(
        ProviderPlanRequest {
            target_id: StableId::parse("local").unwrap(),
            target_identity_digest: digest('9'),
            composed_loadout_digest: digest('a'),
            observed_digest: digest('b'),
            policy_digest: digest('c'),
            ownership_rules: &rules,
            mapped_side_effects: BTreeSet::new(),
        },
        &[state],
        &artifacts,
        &mut router,
    )
    .expect_err("package route is not installed");

    assert!(matches!(
        error,
        ProviderPlanError::MissingResourcePlanner(ResourceType::Package)
    ));
    assert!(
        !adapter_state.join("operations").exists()
            || fs::read_dir(adapter_state.join("operations"))
                .unwrap()
                .next()
                .is_none(),
        "filesystem operation was registered before router preflight"
    );
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_resource_routes_are_rejected_as_ambiguous() {
    let mut first = NoopPackagePlanner::new("packages");
    let mut second = NoopPackagePlanner::new("packages");
    let error = match ProviderResourceRouter::new(vec![
        ProviderPlannerRoute::Package(&mut first),
        ProviderPlannerRoute::Package(&mut second),
    ]) {
        Ok(_) => panic!("ambiguous package route"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        ProviderPlanError::DuplicateResourcePlanner(ResourceType::Package)
    ));
}

#[test]
fn package_artifact_omission_and_tamper_fail_before_registration() {
    let root = temporary_directory("provider-plan-package-artifacts");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let request = || ProviderPlanRequest {
        target_id: StableId::parse("local").unwrap(),
        target_identity_digest: digest('9'),
        composed_loadout_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        ownership_rules: &rules,
        mapped_side_effects: BTreeSet::new(),
    };

    let omitted = mixed_state(&artifacts);
    let mut omitted_resources = omitted.resources;
    let package = omitted_resources
        .iter_mut()
        .find(|resource| resource.resource_type() == ResourceType::Package)
        .unwrap();
    let commonkit_adapters::ResourceIntent::ResolvedPackage(intent) = &mut package.intent else {
        unreachable!()
    };
    intent.artifacts.clear();
    let omitted = ResolvedMaterializedState::finalize(
        omitted.inputs,
        omitted_resources,
        omitted.declared_side_effects,
        omitted.unsupported,
        omitted.capabilities,
    )
    .unwrap();
    let mut omitted_files =
        FileAdapter::open(&root.join("target-omitted"), &root.join("state-omitted")).unwrap();
    let mut omitted_packages = NoopPackagePlanner::new("packages");
    let mut omitted_router = ProviderResourceRouter::new(vec![
        ProviderPlannerRoute::Filesystem(&mut omitted_files),
        ProviderPlannerRoute::Package(&mut omitted_packages),
    ])
    .unwrap();
    let omitted_error = build_resolved_provider_plan_with_router(
        request(),
        &[omitted],
        &artifacts,
        &mut omitted_router,
    )
    .expect_err("package artifact omission");
    assert!(matches!(
        omitted_error,
        ProviderPlanError::PackageResolution(
            commonkit_adapters::PackageResolutionError::ArtifactSetMismatch
        )
    ));

    let tampered = mixed_state(&artifacts);
    let mut tampered_resources = tampered.resources;
    let package = tampered_resources
        .iter_mut()
        .find(|resource| resource.resource_type() == ResourceType::Package)
        .unwrap();
    let commonkit_adapters::ResourceIntent::ResolvedPackage(intent) = &mut package.intent else {
        unreachable!()
    };
    intent.artifacts[0].bytes += 1;
    let tampered = ResolvedMaterializedState::finalize(
        tampered.inputs,
        tampered_resources,
        tampered.declared_side_effects,
        tampered.unsupported,
        tampered.capabilities,
    )
    .unwrap();
    let mut files =
        FileAdapter::open(&root.join("target-tampered"), &root.join("state-tampered")).unwrap();
    let mut packages = NoopPackagePlanner::new("packages");
    let mut router = ProviderResourceRouter::new(vec![
        ProviderPlannerRoute::Filesystem(&mut files),
        ProviderPlannerRoute::Package(&mut packages),
    ])
    .unwrap();
    let tampered_error =
        build_resolved_provider_plan_with_router(request(), &[tampered], &artifacts, &mut router)
            .expect_err("tampered package artifact reference");
    assert!(matches!(
        tampered_error,
        ProviderPlanError::PackageResolution(
            commonkit_adapters::PackageResolutionError::ArtifactSetMismatch
        )
    ));

    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resource_planner_cannot_choose_an_unregistered_adapter_id() {
    let root = temporary_directory("provider-plan-adapter-spoof");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let mixed = mixed_state(&artifacts);
    let package_resources = mixed
        .resources
        .into_iter()
        .filter(|resource| resource.resource_type() == ResourceType::Package)
        .collect();
    let state = ResolvedMaterializedState::finalize(
        mixed.inputs,
        package_resources,
        mixed.declared_side_effects,
        mixed.unsupported,
        mixed.capabilities,
    )
    .unwrap();
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let mut planner = SpoofPackagePlanner {
        declared: StableId::parse("packages").unwrap(),
        returned: StableId::parse("provider-chosen-adapter").unwrap(),
        spoof_resource_binding: false,
    };
    let mut router =
        ProviderResourceRouter::new(vec![ProviderPlannerRoute::Package(&mut planner)]).unwrap();
    let error = build_resolved_provider_plan_with_router(
        ProviderPlanRequest {
            target_id: StableId::parse("local").unwrap(),
            target_identity_digest: digest('9'),
            composed_loadout_digest: digest('a'),
            observed_digest: digest('b'),
            policy_digest: digest('c'),
            ownership_rules: &rules,
            mapped_side_effects: BTreeSet::new(),
        },
        &[state],
        &artifacts,
        &mut router,
    )
    .expect_err("provider-selected adapter ID");
    assert!(matches!(
        error,
        ProviderPlanError::UnexpectedAdapterRoute {
            resource_type: ResourceType::Package,
            ..
        }
    ));
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resource_planner_cannot_spoof_the_routed_resource_binding() {
    let root = temporary_directory("provider-plan-resource-spoof");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let mixed = mixed_state(&artifacts);
    let package_resources = mixed
        .resources
        .into_iter()
        .filter(|resource| resource.resource_type() == ResourceType::Package)
        .collect();
    let state = ResolvedMaterializedState::finalize(
        mixed.inputs,
        package_resources,
        mixed.declared_side_effects,
        mixed.unsupported,
        mixed.capabilities,
    )
    .unwrap();
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let mut planner = SpoofPackagePlanner {
        declared: StableId::parse("packages").unwrap(),
        returned: StableId::parse("packages").unwrap(),
        spoof_resource_binding: true,
    };
    let mut router =
        ProviderResourceRouter::new(vec![ProviderPlannerRoute::Package(&mut planner)]).unwrap();
    let error = build_resolved_provider_plan_with_router(
        ProviderPlanRequest {
            target_id: StableId::parse("local").unwrap(),
            target_identity_digest: digest('9'),
            composed_loadout_digest: digest('a'),
            observed_digest: digest('b'),
            policy_digest: digest('c'),
            ownership_rules: &rules,
            mapped_side_effects: BTreeSet::new(),
        },
        &[state],
        &artifacts,
        &mut router,
    )
    .expect_err("provider-selected resource binding");
    assert!(matches!(
        error,
        ProviderPlanError::UnexpectedResourceBinding {
            resource_type: ResourceType::Package,
            ..
        }
    ));
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_provider_states_build_a_durable_mutation_free_bound_plan() {
    let root = temporary_directory("provider-plan");
    let target = root.join("target");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let native_content = artifacts
        .put(b"native\n", ContentSensitivity::Portable)
        .unwrap();
    let apm_content = artifacts
        .put(b"apm\n", ContentSensitivity::Portable)
        .unwrap();
    let native = state(
        "native",
        "1.0.0",
        "native-v1",
        "home/native.txt",
        native_content,
    );
    let apm = state("apm", "0.25.0", "apm-v1", "agents/apm.txt", apm_content);
    let mut adapter = FileAdapter::open(&target, &root.join("adapter-state")).unwrap();
    let rules = OwnershipRules::new(
        true,
        vec![
            NormalizedManagedPath::parse("home").unwrap(),
            NormalizedManagedPath::parse("agents").unwrap(),
        ],
        vec![NormalizedManagedPath::parse("home/.commonkit").unwrap()],
    )
    .unwrap();

    let request = ProviderPlanRequest {
        target_id: StableId::parse("local").unwrap(),
        target_identity_digest: digest('9'),
        composed_loadout_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        ownership_rules: &rules,
        mapped_side_effects: BTreeSet::new(),
    };
    let plan = build_provider_plan(
        request,
        &[apm.clone(), native.clone()],
        &artifacts,
        &mut adapter,
    )
    .expect("provider plan");

    assert_eq!(plan.operations.len(), 2);
    assert!(!target.join("home/native.txt").exists());
    assert!(!target.join("agents/apm.txt").exists());
    assert!(
        plan.operations
            .iter()
            .all(|operation| operation.adapter_id.as_str() == "files")
    );
    let apm_operation = plan
        .operations
        .iter()
        .find(|operation| operation.resource.managed_path.as_deref() == Some("agents/apm.txt"))
        .expect("APM operation");
    let provenance = apm_operation
        .provenance
        .as_ref()
        .expect("provider resource provenance");
    assert_eq!(provenance.provider_id.as_str(), "apm");
    assert_eq!(provenance.provider_version, "0.25.0");
    assert_eq!(provenance.source, "fixture:agents/apm.txt");
    assert_eq!(provenance.input_digest, apm.inputs.digest().clone());
    for operation in &plan.operations {
        adapter.prepare(operation).unwrap();
        adapter.apply(operation).unwrap();
        adapter.verify(operation).unwrap();
    }
    assert_eq!(
        fs::read(target.join("home/native.txt")).unwrap(),
        b"native\n"
    );
    assert_eq!(fs::read(target.join("agents/apm.txt")).unwrap(), b"apm\n");

    let changed = state(
        "apm",
        "0.25.1",
        "apm-v2",
        "agents/apm.txt",
        artifacts
            .put(b"apm\n", ContentSensitivity::Portable)
            .unwrap(),
    );
    let changed_plan = build_provider_plan(
        ProviderPlanRequest {
            target_id: StableId::parse("local").unwrap(),
            target_identity_digest: digest('9'),
            composed_loadout_digest: digest('a'),
            observed_digest: digest('b'),
            policy_digest: digest('c'),
            ownership_rules: &rules,
            mapped_side_effects: BTreeSet::new(),
        },
        &[changed, native],
        &artifacts,
        &mut FileAdapter::open(&target, &root.join("changed-state")).unwrap(),
    )
    .unwrap();
    assert_ne!(changed_plan.id, plan.id);
    assert_ne!(
        changed_plan.bindings.provider_inputs_digest,
        plan.bindings.provider_inputs_digest
    );

    drop(adapter);
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_policy_failures_stop_before_operation_registration() {
    let root = temporary_directory("provider-plan-policy");
    let target = root.join("target");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let content = artifacts
        .put(b"data", ContentSensitivity::Portable)
        .unwrap();
    let base = state("native", "1.0.0", "native-v1", "outside/file", content);
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let request = || ProviderPlanRequest {
        target_id: StableId::parse("local").unwrap(),
        target_identity_digest: digest('9'),
        composed_loadout_digest: digest('a'),
        observed_digest: digest('b'),
        policy_digest: digest('c'),
        ownership_rules: &rules,
        mapped_side_effects: BTreeSet::new(),
    };
    let error = build_provider_plan(
        request(),
        std::slice::from_ref(&base),
        &artifacts,
        &mut FileAdapter::open(&target, &root.join("outside-state")).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(error, ProviderPlanError::Ownership(_)));

    let unsupported = MaterializedState::finalize(
        base.inputs.clone(),
        vec![],
        vec![],
        vec![UnsupportedCapability {
            source: "run_once.sh".into(),
            capability: "script".into(),
            remediation: "remove it".into(),
        }],
    )
    .unwrap();
    let error = build_provider_plan(
        request(),
        &[unsupported],
        &artifacts,
        &mut FileAdapter::open(&target, &root.join("unsupported-state")).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ProviderPlanError::UnsupportedCapability { .. }
    ));

    let side_effect = MaterializedState::finalize(
        base.inputs,
        vec![],
        vec![DeclaredSideEffect::ServiceRestart {
            service: "editor".into(),
        }],
        vec![],
    )
    .unwrap();
    let error = build_provider_plan(
        request(),
        &[side_effect],
        &artifacts,
        &mut FileAdapter::open(&target, &root.join("effect-state")).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(error, ProviderPlanError::UnmappedSideEffect(_)));
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn protected_roots_are_deterministically_bound_into_plan_authority() {
    let root = temporary_directory("provider-plan-protected-roots");
    let target = root.join("target");
    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let desired = state(
        "native",
        "1.0.0",
        "native-v1",
        "home/native.txt",
        artifacts
            .put(b"native\n", ContentSensitivity::Portable)
            .unwrap(),
    );
    let roots = |protected: &[&str]| {
        OwnershipRules::new(
            true,
            vec![NormalizedManagedPath::parse("home").unwrap()],
            protected
                .iter()
                .map(|path| NormalizedManagedPath::parse(*path).unwrap())
                .collect(),
        )
        .unwrap()
    };
    let build = |rules: &OwnershipRules, state_dir: &str| {
        build_provider_plan(
            ProviderPlanRequest {
                target_id: StableId::parse("local").unwrap(),
                target_identity_digest: digest('9'),
                composed_loadout_digest: digest('a'),
                observed_digest: digest('b'),
                policy_digest: digest('c'),
                ownership_rules: rules,
                mapped_side_effects: BTreeSet::new(),
            },
            std::slice::from_ref(&desired),
            &artifacts,
            &mut FileAdapter::open(&target, &root.join(state_dir)).unwrap(),
        )
        .unwrap()
    };

    let forward = build(&roots(&["home/.commonkit", "home/.ssh/control"]), "forward");
    let reverse = build(&roots(&["home/.ssh/control", "home/.commonkit"]), "reverse");
    assert_eq!(forward.id, reverse.id);
    assert_eq!(
        forward.bindings.ownership_map_digest,
        reverse.bindings.ownership_map_digest
    );

    let changed = build(&roots(&["home/.commonkit"]), "changed");
    assert_ne!(forward.id, changed.id);
    assert_ne!(
        forward.bindings.ownership_map_digest,
        changed.bindings.ownership_map_digest
    );

    drop(artifacts);
    fs::remove_dir_all(root).unwrap();
}

fn temporary_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "commonkit-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

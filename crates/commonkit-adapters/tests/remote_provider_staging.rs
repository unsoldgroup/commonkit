use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use commonkit_adapters::{
    ArtifactEvidence, ArtifactStore, ContentSensitivity, DesiredStateProvider,
    ExactProviderVersion, FilesystemIntent, ManagerBindingV1, MaterializedState,
    NormalizedManagedPath, NormalizedResource, OfflineInstallRecipeV1, PackageArtifactV1,
    PackageDesiredIntent, PackageObservationV1, PackageResolutionAuthority, PackageResolutionV1,
    PackageSourceRegistry, PackageTargetV1, ProviderContext, ProviderFailure, ProviderInputs,
    ProviderWorkspace, RemoteProviderStager, ResolvedMaterializedState, ResolvedPackage,
    ResolvedPackageIntent, ResourceProvenance, SourceBindingV1, SshFilesystemRequest,
    SshFilesystemResponse, SshFilesystemTransport, TargetFilesystemError,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, PackageSelector, SchemaVersion, SecurityPolicy,
    Sha256Digest, StableId,
};

fn package_policy() -> SecurityPolicy {
    SecurityPolicy {
        allowlists: BTreeMap::from([(
            StableId::parse("package_sources").unwrap(),
            BTreeSet::from(["homebrew-core".into()]),
        )]),
        ..SecurityPolicy::default()
    }
}

struct FixtureProvider {
    id: StableId,
    content: Vec<u8>,
    sensitivity: ContentSensitivity,
    unsupported: bool,
}

impl DesiredStateProvider for FixtureProvider {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn inspect_inputs(&self, context: &ProviderContext) -> Result<ProviderInputs, ProviderFailure> {
        ProviderInputs::new(
            self.id.clone(),
            ExactProviderVersion::parse("1.2.3")?,
            "fixture.v1".into(),
            BTreeMap::from([("policy".into(), context.policy_digest.clone())]),
            vec![],
        )
        .map_err(Into::into)
    }

    fn materialize(
        &self,
        context: &ProviderContext,
        _workspace: &ProviderWorkspace,
        artifacts: &ArtifactStore,
    ) -> Result<MaterializedState, ProviderFailure> {
        let inputs = self.inspect_inputs(context)?;
        let content = artifacts
            .put(&self.content, self.sensitivity)
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
        let resource = NormalizedResource {
            intent: (FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/.config/agent/settings.json").unwrap(),
                content,
                mode: None,
                expected_before: None,
            })
            .into(),
            provenance: ResourceProvenance {
                provider_id: self.id.clone(),
                provider_version: "1.2.3".into(),
                input_digest: inputs.input_set_digest.clone(),
                source: "fixture".into(),
            },
        };
        MaterializedState::finalize(
            inputs,
            vec![resource],
            vec![],
            self.unsupported
                .then(|| commonkit_adapters::UnsupportedCapability {
                    source: "fixture".into(),
                    capability: "script".into(),
                    remediation: "remove it".into(),
                })
                .into_iter()
                .collect(),
        )
        .map_err(Into::into)
    }
}

#[derive(Default)]
struct RecordingTransport {
    requests: Vec<SshFilesystemRequest>,
}

impl SshFilesystemTransport for RecordingTransport {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        self.requests.push(request.clone());
        match request {
            SshFilesystemRequest::StageArtifact { digest, .. } => {
                Ok(SshFilesystemResponse::ArtifactStaged { digest })
            }
            SshFilesystemRequest::VerifyArtifact { digest, .. } => {
                Ok(SshFilesystemResponse::ArtifactVerified { digest })
            }
            _ => panic!("unexpected request"),
        }
    }
}

fn digest(c: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", c.to_string().repeat(64))).unwrap()
}

fn context() -> ProviderContext {
    ProviderContext {
        target_id: StableId::parse("remote-linux").unwrap(),
        platform: "linux".into(),
        architecture: "x86_64".into(),
        policy_digest: digest('a'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::from([("hostname".into(), digest('b'))]),
    }
}

fn roots(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "commonkit-remote-provider-{name}-{}",
        std::process::id()
    ));
    let staging = root.join("staging");
    let artifacts = root.join("artifacts");
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&artifacts).unwrap();
    (root, staging, artifacts)
}

fn resolved_package_state(store: &ArtifactStore) -> ResolvedMaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "provider.v2".into(),
        BTreeMap::from([("policy".into(), digest('a'))]),
        vec!["package".into()],
    )
    .unwrap();
    let declaration = PackageDeclaration {
        id: StableId::parse("ripgrep").unwrap(),
        version: "14.1.1".into(),
        manager: PackageManager::Homebrew,
        source: StableId::parse("homebrew-core").unwrap(),
        selector: Some(PackageSelector::HomebrewFormula {
            name: "ripgrep".into(),
        }),
    };
    let registry = PackageSourceRegistry::builtin().unwrap();
    let source = SourceBindingV1 {
        source_id: declaration.source.clone(),
        registry_definition_digest: registry
            .source_definition_digest(&declaration.source)
            .unwrap(),
        canonical_repository: "https://github.com/Homebrew/homebrew-core".into(),
        repository_revision: Some("0123456789abcdef0123456789abcdef01234567".into()),
        signed_metadata: vec![ArtifactEvidence {
            authority: StableId::parse("homebrew").unwrap(),
            metadata_digest: digest('2'),
            signature_digest: digest('3'),
        }],
    };
    let package_bytes = b"package";
    let package_artifact = store
        .put(package_bytes, ContentSensitivity::Portable)
        .unwrap();
    let resolution = PackageResolutionV1 {
        schema_version: SchemaVersion(1),
        declaration: declaration.clone(),
        target: PackageTargetV1 {
            os: "linux".into(),
            os_version: "24.04".into(),
            distro_id: Some("ubuntu".into()),
            distro_version: Some("24.04".into()),
            codename: Some("noble".into()),
            arch: "x86_64".into(),
            libc: Some("glibc".into()),
            manager_prefix: Some("/home/linuxbrew/.linuxbrew".into()),
        },
        manager: ManagerBindingV1 {
            manager: PackageManager::Homebrew,
            version: "4.5.0".into(),
            executable_digest: digest('e'),
            config_digest: digest('f'),
        },
        source: source.clone(),
        before: PackageObservationV1 {
            installed_versions: BTreeSet::new(),
        },
        closure: vec![ResolvedPackage {
            declaration: declaration.clone(),
            source: source.clone(),
        }],
        artifacts: vec![PackageArtifactV1 {
            role: StableId::parse("bottle").unwrap(),
            content: package_artifact.clone(),
            upstream_checksum: package_artifact.digest.clone(),
            size: package_artifact.bytes,
            materialization_key: StableId::parse("ripgrep-bottle").unwrap(),
            source_metadata_digest: source.metadata_digest().unwrap(),
        }],
        recipe: OfflineInstallRecipeV1::HomebrewBottle {
            artifact_roles: BTreeSet::from([StableId::parse("bottle").unwrap()]),
        },
    };
    let resolution_reference = store
        .put(
            &serde_json::to_vec(&resolution).unwrap(),
            ContentSensitivity::Portable,
        )
        .unwrap();
    ResolvedMaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: ResolvedPackageIntent {
                declaration,
                resolution: resolution_reference,
                artifacts: vec![package_artifact],
            }
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id,
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest,
                source: "package:ripgrep".into(),
            },
        }],
        vec![],
        vec![],
        vec![],
    )
    .unwrap()
}

#[test]
fn resolved_remote_staging_rejects_stale_manager_before_remote_contact() {
    let (root, _staging, artifacts) = roots("resolved-stale-manager");
    let store = ArtifactStore::open(&artifacts).unwrap();
    let state = resolved_package_state(&store);
    let registry = PackageSourceRegistry::builtin().unwrap();
    let target = PackageTargetV1 {
        os: "linux".into(),
        os_version: "24.04".into(),
        distro_id: Some("ubuntu".into()),
        distro_version: Some("24.04".into()),
        codename: Some("noble".into()),
        arch: "x86_64".into(),
        libc: Some("glibc".into()),
        manager_prefix: Some("/home/linuxbrew/.linuxbrew".into()),
    };
    let stale_manager = ManagerBindingV1 {
        manager: PackageManager::Homebrew,
        version: "4.5.1".into(),
        executable_digest: digest('e'),
        config_digest: digest('f'),
    };
    let authority =
        PackageResolutionAuthority::new(&target, &stale_manager, &registry, &package_policy())
            .unwrap();
    let mut transport = RecordingTransport::default();
    let error = RemoteProviderStager::new(&mut transport)
        .stage_resolved_materialized(
            &state,
            &authority,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-stale-manager").unwrap(),
        )
        .expect_err("stale manager binding");

    assert!(matches!(
        error,
        commonkit_adapters::RemoteProviderStagingError::PackageResolution(
            commonkit_adapters::PackageResolutionError::ManagerBindingMismatch
        )
    ));
    assert!(transport.requests.is_empty());

    let current_manager = ManagerBindingV1 {
        manager: PackageManager::Homebrew,
        version: "4.5.0".into(),
        executable_digest: digest('e'),
        config_digest: digest('f'),
    };
    let removed_policy_authority = PackageResolutionAuthority::new(
        &target,
        &current_manager,
        &registry,
        &SecurityPolicy::default(),
    )
    .unwrap();
    let mut transport = RecordingTransport::default();
    let error = RemoteProviderStager::new(&mut transport)
        .stage_resolved_materialized(
            &state,
            &removed_policy_authority,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-removed-policy").unwrap(),
        )
        .expect_err("removed source policy must fail before remote contact");
    assert!(matches!(
        error,
        commonkit_adapters::RemoteProviderStagingError::PackageResolution(
            commonkit_adapters::PackageResolutionError::PackageSourceNotAllowed { .. }
        )
    ));
    assert!(transport.requests.is_empty());

    let current_authority =
        PackageResolutionAuthority::new(&target, &current_manager, &registry, &package_policy())
            .unwrap();
    assert_ne!(
        removed_policy_authority.digest(),
        current_authority.digest(),
        "effective package-source policy must participate in authority binding"
    );
    let mut transport = RecordingTransport::default();
    let receipt = RemoteProviderStager::new(&mut transport)
        .stage_resolved_materialized(
            &state,
            &current_authority,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-current-authority").unwrap(),
        )
        .unwrap();
    assert_eq!(
        receipt.package_resolution_authority_digest,
        Some(current_authority.digest().clone())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn materializes_locally_then_stages_and_verifies_portable_artifacts_over_typed_ssh() {
    let (root, staging, artifacts) = roots("happy");
    let workspace = ProviderWorkspace::open(&staging, &[]).unwrap();
    let store = ArtifactStore::open(&artifacts).unwrap();
    let provider = FixtureProvider {
        id: StableId::parse("apm").unwrap(),
        content: b"portable".to_vec(),
        sensitivity: ContentSensitivity::Portable,
        unsupported: false,
    };
    let mut transport = RecordingTransport::default();
    let result = RemoteProviderStager::new(&mut transport)
        .materialize_and_stage(
            &provider,
            &context(),
            &workspace,
            &store,
            StableId::parse("run-1").unwrap(),
        )
        .unwrap();
    assert_eq!(result.provider_id, StableId::parse("apm").unwrap());
    assert_eq!(result.artifact_digests.len(), 1);
    assert!(matches!(
        transport.requests[0],
        SshFilesystemRequest::StageArtifact { .. }
    ));
    assert!(matches!(
        transport.requests[1],
        SshFilesystemRequest::VerifyArtifact { .. }
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stages_pipeline_output_without_re_running_the_provider() {
    let (root, staging, artifacts) = roots("pipeline-output");
    let workspace = ProviderWorkspace::open(&staging, &[]).unwrap();
    let store = ArtifactStore::open(&artifacts).unwrap();
    let provider = FixtureProvider {
        id: StableId::parse("chezmoi").unwrap(),
        content: b"portable pipeline output".to_vec(),
        sensitivity: ContentSensitivity::Portable,
        unsupported: false,
    };
    let state = provider
        .materialize(&context(), &workspace, &store)
        .unwrap();
    let mut transport = RecordingTransport::default();
    let receipt = RemoteProviderStager::new(&mut transport)
        .stage_materialized(
            &state,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-pipeline").unwrap(),
        )
        .unwrap();
    assert_eq!(receipt.materialized_state_digest, state.digest);
    assert_eq!(receipt.target_id.as_str(), "remote-linux");
    assert_eq!(transport.requests.len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_package_desire_cannot_stage_controller_owned_resolution_artifacts() {
    let (root, _staging, artifacts) = roots("package-output");
    let store = ArtifactStore::open(&artifacts).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "provider.v2".into(),
        BTreeMap::from([("policy".into(), digest('a'))]),
        vec!["package".into()],
    )
    .unwrap();
    let state = MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: PackageDesiredIntent::new(PackageDeclaration {
                id: StableId::parse("ripgrep").unwrap(),
                version: "14.1.1".into(),
                manager: PackageManager::Homebrew,
                source: StableId::parse("homebrew-core").unwrap(),
                selector: None,
            })
            .unwrap()
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id,
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest,
                source: "package:ripgrep".into(),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let mut transport = RecordingTransport::default();
    let receipt = RemoteProviderStager::new(&mut transport)
        .stage_materialized(
            &state,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-package").unwrap(),
        )
        .unwrap();

    assert!(receipt.artifact_digests.is_empty());
    assert!(transport.requests.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_metadata_for_one_digest_fails_before_remote_staging() {
    let (root, _staging, artifacts) = roots("conflicting-reference-metadata");
    let store = ArtifactStore::open(&artifacts).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "provider.v2".into(),
        BTreeMap::from([("policy".into(), digest('a'))]),
        vec!["filesystem".into()],
    )
    .unwrap();
    let resolution = store
        .put(b"same digest", ContentSensitivity::Portable)
        .unwrap();
    let mut conflicting = resolution.clone();
    conflicting.bytes += 1;
    let state = MaterializedState::finalize(
        inputs.clone(),
        vec![
            NormalizedResource {
                intent: FilesystemIntent::File {
                    path: NormalizedManagedPath::parse("home/first").unwrap(),
                    content: resolution,
                    mode: None,
                    expected_before: None,
                }
                .into(),
                provenance: ResourceProvenance {
                    provider_id: inputs.provider_id.clone(),
                    provider_version: inputs.provider_version.to_string(),
                    input_digest: inputs.input_set_digest.clone(),
                    source: "file:first".into(),
                },
            },
            NormalizedResource {
                intent: FilesystemIntent::File {
                    path: NormalizedManagedPath::parse("home/second").unwrap(),
                    content: conflicting,
                    mode: None,
                    expected_before: None,
                }
                .into(),
                provenance: ResourceProvenance {
                    provider_id: inputs.provider_id,
                    provider_version: inputs.provider_version.to_string(),
                    input_digest: inputs.input_set_digest,
                    source: "file:second".into(),
                },
            },
        ],
        vec![],
        vec![],
    )
    .unwrap();
    let mut transport = RecordingTransport::default();
    let error = RemoteProviderStager::new(&mut transport)
        .stage_materialized(
            &state,
            &store,
            StableId::parse("remote-linux").unwrap(),
            StableId::parse("run-conflict").unwrap(),
        )
        .expect_err("conflicting metadata for one digest");

    assert!(matches!(
        error,
        commonkit_adapters::RemoteProviderStagingError::ConflictingArtifactReference { .. }
    ));
    assert!(transport.requests.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_or_sensitive_output_never_reaches_remote_target() {
    for (name, sensitivity, unsupported) in [
        ("unsupported", ContentSensitivity::Portable, true),
        ("sensitive", ContentSensitivity::LocalSensitive, false),
    ] {
        let (root, staging, artifacts) = roots(name);
        let workspace = ProviderWorkspace::open(&staging, &[]).unwrap();
        let store = ArtifactStore::open(&artifacts).unwrap();
        let provider = FixtureProvider {
            id: StableId::parse("chezmoi").unwrap(),
            content: b"secret".to_vec(),
            sensitivity,
            unsupported,
        };
        let mut transport = RecordingTransport::default();
        assert!(
            RemoteProviderStager::new(&mut transport)
                .materialize_and_stage(
                    &provider,
                    &context(),
                    &workspace,
                    &store,
                    StableId::parse("run-2").unwrap(),
                )
                .is_err()
        );
        assert!(transport.requests.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

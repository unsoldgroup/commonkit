use std::collections::BTreeMap;
use std::fs;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, DesiredStateProvider, ExactProviderVersion,
    FilesystemIntent, MaterializedState, NormalizedManagedPath, NormalizedResource,
    PackageDesiredIntent, ProviderContext, ProviderFailure, ProviderInputs, ProviderWorkspace,
    RemoteProviderStager, ResourceProvenance, SshFilesystemRequest, SshFilesystemResponse,
    SshFilesystemTransport, TargetFilesystemError,
};
use commonkit_contracts::{PackageDeclaration, PackageManager, Sha256Digest, StableId};

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

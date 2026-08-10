use std::collections::BTreeMap;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent, NativeProvider,
    NormalizedManagedPath, NormalizedResource, OwnershipRules, ProviderContext, ProviderInputs,
    ProviderPipeline, ResourceProvenance,
};
use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};

fn digest(value: &str) -> Sha256Digest {
    digest_domain_json("test.provider-pipeline.v1", &value).unwrap()
}

fn temporary_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "commonkit-provider-pipeline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn materializes_providers_into_digest_addressed_state_after_ownership_validation() {
    let temporary = temporary_root();
    let artifacts = ArtifactStore::open(temporary.join("artifacts")).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "native.v1".into(),
        BTreeMap::from([("loadout".into(), digest("loadout"))]),
        vec!["files".into()],
    )
    .unwrap();
    let content = artifacts
        .put(b"managed\n", ContentSensitivity::Portable)
        .unwrap();
    let provider = NativeProvider::new(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/editor.conf").unwrap(),
                content,
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
    )
    .unwrap();
    std::fs::create_dir_all(temporary.join("live")).unwrap();
    let pipeline = ProviderPipeline::open(
        temporary.join("pipeline"),
        artifacts,
        OwnershipRules::new(
            true,
            vec![NormalizedManagedPath::parse("home").unwrap()],
            vec![],
        )
        .unwrap(),
        vec![temporary.join("live")],
    )
    .unwrap();
    let context = ProviderContext {
        target_id: StableId::parse("workstation").unwrap(),
        platform: "linux".into(),
        architecture: "x86_64".into(),
        policy_digest: digest("policy"),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    };

    let output = pipeline.materialize_all(&[&provider], &context).unwrap();
    assert_eq!(output.states.len(), 1);
    assert_eq!(output.states[0].resources.len(), 1);
    assert!(output.state_paths[0].is_file());
    assert!(
        output.state_paths[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(
                output.states[0]
                    .digest
                    .as_str()
                    .trim_start_matches("sha256:")
            )
    );
    let persisted: commonkit_adapters::MaterializedState =
        serde_json::from_slice(&std::fs::read(&output.state_paths[0]).unwrap()).unwrap();
    persisted.verify().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let states = temporary.join("pipeline/states");
        let redirected = temporary.join("redirected-states");
        std::fs::remove_dir_all(&states).unwrap();
        std::fs::create_dir(&redirected).unwrap();
        symlink(&redirected, &states).unwrap();
        let error = pipeline
            .materialize_all(&[&provider], &context)
            .unwrap_err();
        assert!(error.to_string().contains("non-symlink directory"));
        assert_eq!(std::fs::read_dir(redirected).unwrap().count(), 0);
    }
    std::fs::remove_dir_all(temporary).unwrap();
}

use std::collections::BTreeMap;
use std::fs;

use commonkit_adapters::{
    ArtifactStore, DesiredStateProvider, ExactProviderVersion, FilesystemIntent, NativeProvider,
    NormalizedManagedPath, NormalizedResource, ProviderContext, ProviderInputs, ProviderWorkspace,
    ResourceProvenance,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

#[test]
fn native_fallback_materializes_precomposed_resources_deterministically() {
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1").unwrap(),
        "commonkit.native-provider.v1".into(),
        BTreeMap::from([("composition".into(), digest('a'))]),
        vec!["filesystem".into()],
    )
    .unwrap();
    let resource = NormalizedResource {
        intent: FilesystemIntent::Directory {
            path: NormalizedManagedPath::parse("home/.config").unwrap(),
            mode: None,
            exact: false,
        },
        provenance: ResourceProvenance {
            provider_id: StableId::parse("native").unwrap(),
            provider_version: "1".into(),
            input_digest: inputs.digest().clone(),
            source: "layers:/capabilities/files".into(),
        },
    };
    let provider = NativeProvider::new(inputs, vec![resource]).unwrap();
    let root = std::env::temp_dir().join(format!("commonkit-native-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("stage")).unwrap();
    fs::create_dir_all(root.join("live")).unwrap();
    let workspace = ProviderWorkspace::open(root.join("stage"), &[root.join("live")]).unwrap();
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let context = ProviderContext {
        target_id: StableId::parse("local").unwrap(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        policy_digest: digest('b'),
        declared_roots: vec![NormalizedManagedPath::parse("home").unwrap()],
        observed_fact_digests: BTreeMap::new(),
    };

    let first = provider
        .materialize(&context, &workspace, &artifacts)
        .unwrap();
    let second = provider
        .materialize(&context, &workspace, &artifacts)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.resources.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

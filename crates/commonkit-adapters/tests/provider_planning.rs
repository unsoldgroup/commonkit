use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, DeclaredSideEffect, ExactProviderVersion, FileAdapter,
    FilesystemIntent, MaterializedState, NormalizedManagedPath, NormalizedResource, OwnershipRules,
    ProviderInputs, ProviderPlanError, ProviderPlanRequest, ResourceProvenance,
    UnsupportedCapability, build_provider_plan,
};
use commonkit_contracts::{Sha256Digest, StableId};
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
            },
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

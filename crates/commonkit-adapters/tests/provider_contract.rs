use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use commonkit_adapters::{
    DeclaredSideEffect, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, ProviderInputs, ProviderWorkspace,
    ResourceProvenance, UnsupportedCapability,
};
use commonkit_contracts::{Sha256Digest, StableId};

fn digest(seed: char) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", seed.to_string().repeat(64))).unwrap()
}

fn provider_inputs() -> ProviderInputs {
    ProviderInputs::new(
        StableId::parse("chezmoi").unwrap(),
        ExactProviderVersion::parse("2.70.4").unwrap(),
        "provider.v1".into(),
        BTreeMap::from([
            ("config".into(), digest('a')),
            ("source".into(), digest('b')),
        ]),
        vec!["filesystem".into()],
    )
    .unwrap()
}

fn resource(path: &str) -> NormalizedResource {
    NormalizedResource {
        intent: FilesystemIntent::Directory {
            path: NormalizedManagedPath::parse(path).unwrap(),
            mode: None,
            exact: false,
        },
        provenance: ResourceProvenance {
            provider_id: StableId::parse("chezmoi").unwrap(),
            provider_version: "2.70.4".into(),
            input_digest: provider_inputs().digest().clone(),
            source: format!("source:{path}"),
        },
    }
}

#[test]
fn materialized_state_is_deterministic_and_reconstructable() {
    let inputs = provider_inputs();
    let first = MaterializedState::finalize(
        inputs.clone(),
        vec![resource("home/z"), resource("home/a")],
        vec![DeclaredSideEffect::ServiceRestart {
            service: "editor".into(),
        }],
        vec![UnsupportedCapability {
            source: "run_once_setup.sh".into(),
            capability: "script".into(),
            remediation: "remove the script from the managed source".into(),
        }],
    )
    .unwrap();
    let reordered = MaterializedState::finalize(
        inputs,
        vec![resource("home/a"), resource("home/z")],
        first.declared_side_effects.clone(),
        first.unsupported.clone(),
    )
    .unwrap();

    assert_eq!(first.digest, reordered.digest);
    assert_eq!(first.resources, reordered.resources);

    let encoded = serde_json::to_vec(&first).unwrap();
    let reconstructed: MaterializedState = serde_json::from_slice(&encoded).unwrap();
    reconstructed.verify().unwrap();
    assert_eq!(reconstructed, first);
}

#[test]
fn provider_workspace_cannot_overlap_a_live_or_protected_root() {
    let root = unique_temp_dir("provider-contract");
    let staging = root.join("staging");
    let live = root.join("live");
    fs::create_dir_all(&staging).unwrap();
    fs::create_dir_all(&live).unwrap();

    let workspace = ProviderWorkspace::open(&staging, std::slice::from_ref(&live)).unwrap();
    assert_eq!(workspace.staging_root(), staging.canonicalize().unwrap());
    assert!(ProviderWorkspace::open(&live, std::slice::from_ref(&live)).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn provider_workspace_enforces_private_windows_acls() {
    use commonkit_platform::{PrivatePathKind, verify_private_path};

    let root = unique_temp_dir("provider-workspace-acl");
    let staging = root.join("staging");
    fs::create_dir(&staging).unwrap();

    let workspace = ProviderWorkspace::open(&staging, &[]).unwrap();

    verify_private_path(workspace.staging_root(), PrivatePathKind::Directory).unwrap();
    verify_private_path(workspace.scratch_root(), PrivatePathKind::Directory).unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "commonkit-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

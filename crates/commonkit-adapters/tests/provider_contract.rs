use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use commonkit_adapters::{
    DeclaredSideEffect, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, PackageDesiredIntent, ProviderInputs,
    ProviderWorkspace, ResourceProvenance, UnsupportedCapability,
};
use commonkit_contracts::{
    PackageDeclaration, PackageManager, Sha256Digest, StableId, digest_domain_json,
};
use serde::Serialize;

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
        intent: (FilesystemIntent::Directory {
            path: NormalizedManagedPath::parse(path).unwrap(),
            mode: None,
            exact: false,
        })
        .into(),
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

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacySemantic<'a> {
        inputs_digest: &'a Sha256Digest,
        resources: &'a [NormalizedResource],
        declared_side_effects: &'a [DeclaredSideEffect],
        unsupported: &'a [UnsupportedCapability],
    }
    assert_eq!(
        first.digest,
        digest_domain_json(
            "commonkit.materialized-state.v1",
            &LegacySemantic {
                inputs_digest: first.inputs.digest(),
                resources: &first.resources,
                declared_side_effects: &first.declared_side_effects,
                unsupported: &first.unsupported,
            }
        )
        .unwrap()
    );
    assert!(!String::from_utf8(encoded).unwrap().contains("resourceType"));
}

#[test]
fn mixed_materialized_state_is_deterministic_and_uses_typed_package_data() {
    let inputs = provider_inputs();
    let package = NormalizedResource {
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
            provider_id: inputs.provider_id.clone(),
            provider_version: inputs.provider_version.to_string(),
            input_digest: inputs.digest().clone(),
            source: "packages:ripgrep".into(),
        },
    };
    let filesystem = resource("home/config");
    let forward = MaterializedState::finalize(
        inputs.clone(),
        vec![filesystem.clone(), package.clone()],
        vec![],
        vec![],
    )
    .unwrap();
    let reverse =
        MaterializedState::finalize(inputs, vec![package, filesystem], vec![], vec![]).unwrap();

    assert_eq!(forward, reverse);
    assert!(
        serde_json::to_string(&forward)
            .unwrap()
            .contains("\"type\":\"package\"")
    );
    forward.verify().unwrap();
}

#[test]
fn provider_json_cannot_submit_package_resolution_or_artifacts() {
    let provider_selected_resolution = serde_json::json!({
        "type": "package",
        "declaration": {
            "id": "ripgrep",
            "version": "14.1.1",
            "manager": "homebrew",
            "source": "homebrew-core"
        },
        "resolution": {
            "digest": digest('c'),
            "bytes": 10,
            "sensitivity": "portable"
        },
        "artifacts": [{
            "digest": digest('d'),
            "bytes": 20,
            "sensitivity": "portable"
        }]
    });

    assert!(
        serde_json::from_value::<commonkit_adapters::ProviderResourceIntent>(
            provider_selected_resolution.clone()
        )
        .is_err(),
        "provider wire input must not cross the controller-owned resolution boundary"
    );

    let state = MaterializedState::finalize(
        provider_inputs(),
        vec![resource("home/provider-output")],
        vec![],
        vec![],
    )
    .unwrap();
    let mut state_json = serde_json::to_value(state).unwrap();
    state_json["resources"][0]["intent"] = provider_selected_resolution;
    assert!(
        serde_json::from_value::<MaterializedState>(state_json).is_err(),
        "provider materialized-state decoding must use the desired-only vocabulary"
    );
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

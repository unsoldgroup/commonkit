use commonkit_adapters::*;
use commonkit_contracts::{Sha256Digest, StableId, digest_domain_json};
use commonkit_reconcile::{Adapter, ReceiptStore, ReconcileOutcome, Reconciler};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Remote {
    files: BTreeMap<String, Vec<u8>>,
    directories: BTreeMap<String, Option<u32>>,
    symlinks: BTreeMap<String, String>,
    writes: usize,
    fail_write: Option<usize>,
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
            SshFilesystemRequest::InspectResource { path, .. } => {
                let resource = if let Some(content) = remote.files.get(path.as_str()) {
                    TargetResource::File {
                        content: content.clone(),
                    }
                } else if let Some(mode) = remote.directories.get(path.as_str()) {
                    TargetResource::Directory { mode: *mode }
                } else if let Some(target) = remote.symlinks.get(path.as_str()) {
                    TargetResource::Symlink {
                        target: target.clone(),
                    }
                } else {
                    TargetResource::Absent
                };
                Ok(SshFilesystemResponse::Resource { resource })
            }
            SshFilesystemRequest::ReadFile { path, .. } => Ok(remote
                .files
                .get(path.as_str())
                .cloned()
                .map(|content| SshFilesystemResponse::File { content })
                .unwrap_or(SshFilesystemResponse::Absent)),
            SshFilesystemRequest::WriteFile { path, content, .. } => {
                remote.writes += 1;
                if remote.fail_write == Some(remote.writes) {
                    remote.fail_write = None;
                    return Err(TargetFilesystemError::RemoteFailure {
                        status: 71,
                        message: "simulated crash".into(),
                    });
                }
                remote.directories.remove(path.as_str());
                remote.symlinks.remove(path.as_str());
                remote.files.insert(path.to_string(), content);
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteDirectory { path, mode, .. } => {
                remote.files.remove(path.as_str());
                remote.symlinks.remove(path.as_str());
                remote
                    .directories
                    .insert(path.to_string(), mode.as_ref().map(FileMode::value));
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteSymlink { path, target, .. } => {
                remote.files.remove(path.as_str());
                remote.directories.remove(path.as_str());
                remote
                    .symlinks
                    .insert(path.to_string(), target.as_str().to_owned());
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::Remove { path, .. } => {
                remote.files.remove(path.as_str());
                remote.directories.remove(path.as_str());
                remote.symlinks.remove(path.as_str());
                Ok(SshFilesystemResponse::Applied)
            }
            _ => panic!("unexpected request"),
        }
    }
}
fn digest(value: &str) -> Sha256Digest {
    digest_domain_json("test.ssh-file.v1", &value).unwrap()
}
fn root(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "commonkit-ssh-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn state(artifacts: &ArtifactStore) -> MaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "native.v1".into(),
        BTreeMap::from([("git".into(), digest("rev"))]),
        vec!["files".into()],
    )
    .unwrap();
    let resources = [("home/a", b"a\n".as_slice()), ("home/b", b"b\n".as_slice())]
        .into_iter()
        .map(|(path, bytes)| NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(path).unwrap(),
                content: artifacts.put(bytes, ContentSensitivity::Portable).unwrap(),
                mode: None,
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: path.into(),
            },
        })
        .collect();
    MaterializedState::finalize(inputs, resources, vec![], vec![]).unwrap()
}

fn relative_symlink_state() -> MaterializedState {
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "native.v1".into(),
        BTreeMap::from([("git".into(), digest("rev"))]),
        vec!["files".into()],
    )
    .unwrap();
    let source = NormalizedManagedPath::parse("home/.agents/skills/tool").unwrap();
    let link = NormalizedManagedPath::parse("home/.codex/skills/tool").unwrap();
    let resources = vec![
        NormalizedResource {
            intent: FilesystemIntent::Directory {
                path: source,
                mode: Some(FileMode::parse(0o700).unwrap()),
                exact: false,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "skills/tool".into(),
            },
        },
        NormalizedResource {
            intent: FilesystemIntent::Symlink {
                path: link.clone(),
                target: SafeSymlinkTarget::parse(&link, "../../.agents/skills/tool").unwrap(),
                expected_before: None,
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "codex/skills/tool".into(),
            },
        },
    ];
    MaterializedState::finalize(inputs, resources, vec![], vec![]).unwrap()
}
fn build(
    root: &std::path::Path,
    transport: Memory,
    desired: &MaterializedState,
    artifacts: &ArtifactStore,
) -> (commonkit_contracts::Plan, SshFileAdapter<Memory>) {
    let mut adapter = SshFileAdapter::open(
        StableId::parse("home-root").unwrap(),
        root.join("adapter"),
        transport,
    )
    .unwrap();
    let observed = adapter
        .observed_state_digest(desired.resources.iter().map(|r| &r.intent))
        .unwrap();
    let rules = OwnershipRules::new(
        true,
        vec![NormalizedManagedPath::parse("home").unwrap()],
        vec![],
    )
    .unwrap();
    let plan = build_provider_plan(
        ProviderPlanRequest {
            target_id: StableId::parse("remote-linux").unwrap(),
            target_identity_digest: digest("target"),
            composed_loadout_digest: digest("loadout"),
            observed_digest: observed,
            policy_digest: digest("policy"),
            ownership_rules: &rules,
            mapped_side_effects: BTreeSet::new(),
        },
        std::slice::from_ref(desired),
        artifacts,
        &mut adapter,
    )
    .unwrap();
    (plan, adapter)
}
fn execute(
    root: &std::path::Path,
    plan: &commonkit_contracts::Plan,
    adapter: SshFileAdapter<Memory>,
    run: &str,
) -> ReconcileOutcome {
    let receipts = ReceiptStore::open(root).unwrap();
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(adapter)];
    Reconciler::with_store(&receipts)
        .execute(plan, StableId::parse(run).unwrap(), &mut adapters)
        .unwrap()
}

#[test]
fn stale_plan_is_rejected_crash_rolls_back_and_fresh_adapter_verifies() {
    let temp = root("transaction");
    let artifacts = ArtifactStore::open(temp.join("artifacts")).unwrap();
    let desired = state(&artifacts);
    let stale_remote = Memory::default();
    let (stale, adapter) = build(
        &temp.join("stale"),
        stale_remote.clone(),
        &desired,
        &artifacts,
    );
    stale_remote
        .0
        .lock()
        .unwrap()
        .files
        .insert("home/a".into(), b"hand edit".to_vec());
    assert_eq!(
        execute(&temp.join("stale-receipts"), &stale, adapter, "stale-run"),
        ReconcileOutcome::Canceled
    );
    assert_eq!(stale_remote.0.lock().unwrap().files["home/a"], b"hand edit");

    let remote = Memory::default();
    remote.0.lock().unwrap().fail_write = Some(2);
    let (crash, adapter) = build(&temp.join("crash"), remote.clone(), &desired, &artifacts);
    assert_eq!(
        execute(&temp.join("crash-receipts"), &crash, adapter, "crash-run"),
        ReconcileOutcome::RolledBack
    );
    assert!(remote.0.lock().unwrap().files.is_empty());

    let (success, adapter) = build(&temp.join("success"), remote.clone(), &desired, &artifacts);
    assert_eq!(
        execute(
            &temp.join("success-receipts"),
            &success,
            adapter,
            "success-run"
        ),
        ReconcileOutcome::Succeeded
    );
    let mut fresh = SshFileAdapter::open(
        StableId::parse("home-root").unwrap(),
        temp.join("success/adapter"),
        remote,
    )
    .unwrap();
    for operation in &success.operations {
        fresh.verify(operation).unwrap();
    }
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn relative_directories_and_symlinks_plan_apply_and_verify_over_typed_ssh() {
    let temp = root("relative-symlinks");
    let artifacts = ArtifactStore::open(temp.join("artifacts")).unwrap();
    let desired = relative_symlink_state();
    let remote = Memory::default();
    let (plan, adapter) = build(&temp.join("apply"), remote.clone(), &desired, &artifacts);

    assert_eq!(plan.operations.len(), 2);
    assert_eq!(
        execute(&temp.join("receipts"), &plan, adapter, "relative-run"),
        ReconcileOutcome::Succeeded
    );
    let resources = remote.0.lock().unwrap();
    assert_eq!(
        resources.directories["home/.agents/skills/tool"],
        Some(0o700)
    );
    assert_eq!(
        resources.symlinks["home/.codex/skills/tool"],
        "../../.agents/skills/tool"
    );
    drop(resources);

    let mut fresh = SshFileAdapter::open(
        StableId::parse("home-root").unwrap(),
        temp.join("apply/adapter"),
        remote,
    )
    .unwrap();
    for operation in &plan.operations {
        fresh.verify(operation).unwrap();
    }
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn unsafe_remote_symlink_preimages_fail_before_planning() {
    let temp = root("unsafe-symlink-preimage");
    let desired = relative_symlink_state();
    let remote = Memory::default();
    remote.0.lock().unwrap().symlinks.insert(
        "home/.codex/skills/tool".into(),
        "/outside/absolute-target".into(),
    );
    let mut adapter =
        SshFileAdapter::open(StableId::parse("home-root").unwrap(), &temp, remote).unwrap();

    assert!(matches!(
        adapter.observed_state_digest(desired.resources.iter().map(|resource| &resource.intent)),
        Err(SshFileAdapterError::Resource(_))
    ));

    std::fs::remove_dir_all(temp).unwrap();
}

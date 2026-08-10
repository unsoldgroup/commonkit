use std::collections::BTreeMap;

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, NormalizedResource, ProviderInputs, ResourceProvenance,
};
use commonkit_contracts::{StableId, digest_domain_json};
use commonkit_reconcile::PlanStore;
use commonkit_reconcile::{Adapter, ReceiptStore, ReconcileOutcome, Reconciler};
use commonkit_service::{
    ControlError, ControlPlane, ExecutionResult, PlanExecutor, ProductionDomainRegistry, SyncDomain,
};
use commonkit_snapshots::{
    AuthorityStore, DatabaseId, PortableAuthorityStore, ProcessGitAuthorityPublisher,
    PromotionPlan, PublisherFailpoint, SnapshotError, XChaCha20Cipher,
};
use sha2::{Digest, Sha256};

struct UnexpectedExecutor;

impl PlanExecutor for UnexpectedExecutor {
    fn execute(
        &self,
        _plan: &commonkit_contracts::Plan,
        _confirmation_id: &StableId,
    ) -> ExecutionResult {
        panic!("rejected apply must not reach the executor")
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    path: std::path::PathBuf,
    kind: &'static str,
    bytes: Vec<u8>,
    mode: u32,
    len: u64,
    readonly: bool,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    unix_identity: (u64, u64, u32, u32, u64, i64, i64, i64, i64),
    #[cfg(unix)]
    extended_attributes: Vec<(Vec<u8>, Vec<u8>)>,
}

fn filesystem_snapshot(root: &std::path::Path) -> Vec<TreeEntry> {
    fn visit(base: &std::path::Path, path: &std::path::Path, entries: &mut Vec<TreeEntry>) {
        let mut children = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            let initial_metadata = std::fs::symlink_metadata(&child).unwrap();
            let file_type = initial_metadata.file_type();
            let (kind, bytes) = if file_type.is_dir() {
                ("directory", Vec::new())
            } else if file_type.is_symlink() {
                (
                    "symlink",
                    std::fs::read_link(&child)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                )
            } else {
                ("file", std::fs::read(&child).unwrap())
            };
            let metadata = std::fs::symlink_metadata(&child).unwrap();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = u32::from(metadata.permissions().readonly());
            #[cfg(unix)]
            let unix_identity = {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.uid(),
                    metadata.gid(),
                    metadata.nlink(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            };
            entries.push(TreeEntry {
                path: child.strip_prefix(base).unwrap().to_owned(),
                kind,
                bytes,
                mode,
                len: metadata.len(),
                readonly: metadata.permissions().readonly(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
                #[cfg(unix)]
                unix_identity,
                #[cfg(unix)]
                extended_attributes: read_xattrs(&child),
            });
            if file_type.is_dir() {
                visit(base, &child, entries);
            }
        }
    }

    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    let metadata = std::fs::symlink_metadata(root).unwrap();
    #[cfg(unix)]
    let unix_identity = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.uid(),
            metadata.gid(),
            metadata.nlink(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    entries.push(TreeEntry {
        path: std::path::PathBuf::from("."),
        kind: "directory",
        bytes: Vec::new(),
        mode,
        len: metadata.len(),
        readonly: metadata.permissions().readonly(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        unix_identity,
        #[cfg(unix)]
        extended_attributes: read_xattrs(root),
    });
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

#[cfg(unix)]
fn read_xattrs(path: &std::path::Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return Vec::new();
    };
    #[cfg(target_os = "macos")]
    unsafe fn list(path: *const libc::c_char, buffer: *mut libc::c_char, size: usize) -> isize {
        unsafe { libc::listxattr(path, buffer, size, 0) }
    }
    #[cfg(not(target_os = "macos"))]
    unsafe fn list(path: *const libc::c_char, buffer: *mut libc::c_char, size: usize) -> isize {
        unsafe { libc::listxattr(path, buffer, size) }
    }
    let size = unsafe { list(path.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return Vec::new();
    }
    let mut names = vec![0_u8; size as usize];
    if unsafe { list(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) } < 0 {
        return Vec::new();
    }
    let mut attributes = Vec::new();
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let Ok(name_c) = CString::new(name) else {
            continue;
        };
        #[cfg(target_os = "macos")]
        unsafe fn get(
            path: *const libc::c_char,
            name: *const libc::c_char,
            buffer: *mut libc::c_void,
            size: usize,
        ) -> isize {
            unsafe { libc::getxattr(path, name, buffer, size, 0, 0) }
        }
        #[cfg(not(target_os = "macos"))]
        unsafe fn get(
            path: *const libc::c_char,
            name: *const libc::c_char,
            buffer: *mut libc::c_void,
            size: usize,
        ) -> isize {
            unsafe { libc::getxattr(path, name, buffer, size) }
        }
        let value_size = unsafe { get(path.as_ptr(), name_c.as_ptr(), std::ptr::null_mut(), 0) };
        if value_size < 0 {
            continue;
        }
        let mut value = vec![0_u8; value_size as usize];
        if unsafe {
            get(
                path.as_ptr(),
                name_c.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        } == value_size
        {
            attributes.push((name.to_vec(), value));
        }
    }
    attributes.sort();
    attributes
}

#[cfg(unix)]
#[test]
fn registry_startup_recovers_snapshot_transactions_with_configured_service_lifecycle() {
    use commonkit_snapshots::{
        DatabaseId, DatabaseLifecycle, DurableRestore, InMemoryObjectStore, RestoreFailpoint,
        RestorePlan, SnapshotError, SnapshotService, StaticBackup, XChaCha20Cipher,
        manifest_digest,
    };
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    struct InitialLifecycle;
    impl DatabaseLifecycle for InitialLifecycle {
        fn stop(&mut self) -> Result<(), SnapshotError> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), SnapshotError> {
            Ok(())
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let secret = b"target-local-key";
    let key_path = root.join("key");
    std::fs::write(&key_path, secret).unwrap();
    let key: [u8; 32] = Sha256::digest(secret).into();
    let cipher = XChaCha20Cipher::new(key);
    let mut objects = InMemoryObjectStore::default();
    let database_id = DatabaseId::new("context-mode").unwrap();
    let manifest = SnapshotService::new(&cipher)
        .snapshot(
            &database_id,
            "local",
            None,
            &StaticBackup::new("file", b"restored".to_vec()),
            &mut objects,
        )
        .unwrap();
    let database = root.join("context.db");
    std::fs::write(&database, b"original").unwrap();
    DurableRestore::open(root.join("snapshots/transactions"), &cipher)
        .unwrap()
        .execute(
            RestorePlan {
                schema: "commonkit.restore-plan.v1".into(),
                run_id: "startup-recovery".into(),
                snapshot_id: "snapshot-1".into(),
                manifest_digest: manifest_digest(&manifest).unwrap(),
                database: database_id,
                expected_content_digest: manifest.content_digest.clone(),
            },
            &manifest,
            &objects,
            &database,
            &mut InitialLifecycle,
            RestoreFailpoint::AfterSwap,
        )
        .unwrap_err();

    let lifecycle_log = root.join("lifecycle.log");
    let script = root.join("lifecycle.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            lifecycle_log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let lifecycle = serde_json::json!({
        "stop": {"executable": script, "args":["stop"]},
        "start": {"executable": script, "args":["start"]}
    });
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":database, "targetId":"local", "observedPaths":{"workstation-b":root.join("context-candidate.db")}, "format":"file", "lifecycle":lifecycle}]
            }
        }),
    );
    ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    assert_eq!(std::fs::read(root.join("context.db")).unwrap(), b"original");
    assert_eq!(
        std::fs::read_to_string(lifecycle_log).unwrap(),
        "stop\nstart\n"
    );
}

fn snapshot_git_authority(repository: &std::path::Path) -> serde_json::Value {
    fn git(repository: &std::path::Path, args: &[&str]) -> String {
        let executable = git_executable();
        let output = std::process::Command::new(executable)
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    let executable = git_executable();
    git(repository, &["init", "--initial-branch=main"]);
    git(repository, &["config", "user.name", "CommonKit Test"]);
    git(
        repository,
        &["config", "user.email", "commonkit-test@localhost"],
    );
    std::fs::write(repository.join(".commonkit-git-seed"), b"seed\n").unwrap();
    git(repository, &["add", ".commonkit-git-seed"]);
    git(repository, &["commit", "-m", "seed snapshot authority"]);
    let remote = repository.join(".snapshot-authority-remote.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "--initial-branch=main"]);
    git(
        repository,
        &[
            "remote",
            "add",
            "snapshot-authority",
            remote.to_str().unwrap(),
        ],
    );
    git(repository, &["push", "snapshot-authority", "main"]);
    serde_json::json!({
        "executable": executable,
        "repository": repository,
        "trustedRemoteUrl": remote,
        "branch": "main",
        "stagingRoot": repository.join("snapshot-git-staging"),
        "bootstrap": true
    })
}

fn git_executable() -> std::path::PathBuf {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|path| path.canonicalize().ok())
        .expect("absolute git executable")
}

fn write_json(path: &std::path::Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn write_engram_chunks(root: &std::path::Path, chunks: &[(&str, &[u8])]) {
    std::fs::create_dir_all(root.join("chunks")).unwrap();
    write_json(
        &root.join("manifest.json"),
        &serde_json::json!({
            "version": 1,
            "chunks": chunks.iter().map(|(id, _)| serde_json::json!({
                "id": id, "created_by": "test", "created_at": "2026-08-06T00:00:00Z",
                "sessions": 0, "memories": 1, "prompts": 0
            })).collect::<Vec<_>>()
        }),
    );
    for (id, bytes) in chunks {
        std::fs::write(root.join("chunks").join(format!("{id}.jsonl.gz")), bytes).unwrap();
    }
}

fn write_engram_project_attestation(root: &std::path::Path, chunks: &[(&str, &[u8])]) {
    let manifest = std::fs::read(root.join("manifest.json")).unwrap();
    write_json(
        &root.join("scope-attestation.json"),
        &serde_json::json!({
            "schema": "engram.scope-export.v1",
            "exporterVersion": "1.17.0",
            "projectId": "github.com/unsoldgroup/commonkit",
            "scope": "project",
            "manifestDigest": format!("sha256:{:x}", Sha256::digest(&manifest)),
            "chunks": chunks.iter().map(|(id, bytes)| serde_json::json!({
                "id": id,
                "digest": format!("sha256:{:x}", Sha256::digest(bytes)),
                "bytes": bytes.len()
            })).collect::<Vec<_>>()
        }),
    );
}

#[test]
fn production_engram_reconcile_exchanges_declared_targets_and_persists_a_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let left_target = root.join("left-target");
    let right_target = root.join("right-target");
    write_engram_chunks(&left_target.join("repo/.engram"), &[("11111111", b"left")]);
    write_engram_chunks(
        &right_target.join("repo/.engram"),
        &[("22222222", b"right")],
    );
    let artifacts = root.join("artifacts");
    ArtifactStore::open(&artifacts).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "1".to_owned(),
        BTreeMap::from([(
            "manifest".to_owned(),
            digest_domain_json("test", &"engram-provider").unwrap(),
        )]),
        vec!["files".to_owned()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize(inputs, vec![], vec![], vec![]).unwrap();
    let materialized_path = root.join("materialized.json");
    write_json(&materialized_path, &materialized);
    #[cfg(windows)]
    let engram_executable =
        std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32/cmd.exe");
    #[cfg(not(windows))]
    let engram_executable = std::path::PathBuf::from("/usr/bin/true");
    let sync_config = |id: &str, target: &std::path::Path, adapter: &str| {
        serde_json::json!({
            "targetId": id,
            "targetRoot": target,
            "adapterState": root.join(adapter),
            "providerArtifacts": artifacts,
            "materializedStates": [materialized_path],
            "declaredRoots": ["repo"],
            "protectedRoots": [],
            "caseSensitive": true,
            "targetIdentityDigest": digest_domain_json("test", &id).unwrap(),
            "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
            "policyDigest": digest_domain_json("test", &"policy").unwrap(),
            "engram": {
                "projectId": "github.com/unsoldgroup/commonkit",
                "ownerId": "github:astemarie",
                "projectRoot": "repo",
                "root": "repo/.engram",
                "scope": "project",
                "executable": engram_executable,
                "autoReconcile": if id == "left" {
                    serde_json::json!({"peerTargetId": "right", "intervalSeconds": 60})
                } else {
                    serde_json::Value::Null
                }
            }
        })
    };
    let config_path = root.join("headless-engram.json");
    write_json(
        &config_path,
        &serde_json::json!({
            "sync": sync_config("left", &left_target, "left-adapter"),
            "syncTargets": [sync_config("right", &right_target, "right-adapter")]
        }),
    );
    let receipt_root = root.join("receipts");
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        receipt_root.clone(),
    )
    .unwrap();
    assert_eq!(registry.engram_periodic_interval_seconds(), Some(60));

    let receipt = registry.reconcile_engram_periodic_once().unwrap();
    let receipt = receipt.unwrap();

    assert_eq!(receipt["importsCompleted"], true);
    assert_eq!(
        receipt["reconciliation"]["moved"].as_array().unwrap().len(),
        2
    );
    assert!(
        left_target
            .join("repo/.engram/chunks/22222222.jsonl.gz")
            .is_file()
    );
    assert!(
        right_target
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .is_file()
    );
    assert_eq!(
        std::fs::read_dir(receipt_root.join("engram"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn production_engram_team_reconcile_fails_closed_without_a_grant_and_withdrawal_is_a_no_op() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let left_target = root.join("left-target");
    let right_target = root.join("right-target");
    write_engram_chunks(&left_target.join("repo/.engram"), &[("11111111", b"left")]);
    write_engram_chunks(
        &right_target.join("repo/.engram"),
        &[("22222222", b"right")],
    );
    let artifacts = root.join("artifacts");
    ArtifactStore::open(&artifacts).unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "1".to_owned(),
        BTreeMap::from([(
            "manifest".to_owned(),
            digest_domain_json("test", &"engram-team-provider").unwrap(),
        )]),
        vec!["files".to_owned()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize(inputs, vec![], vec![], vec![]).unwrap();
    let materialized_path = root.join("materialized.json");
    write_json(&materialized_path, &materialized);
    #[cfg(windows)]
    let engram_executable =
        std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32/cmd.exe");
    #[cfg(not(windows))]
    let engram_executable = std::path::PathBuf::from("/usr/bin/true");
    let sync_config = |id: &str, owner: &str, target: &std::path::Path, adapter: &str| {
        serde_json::json!({
            "targetId": id,
            "targetRoot": target,
            "adapterState": root.join(adapter),
            "providerArtifacts": artifacts,
            "materializedStates": [materialized_path],
            "declaredRoots": ["repo"],
            "protectedRoots": [],
            "caseSensitive": true,
            "targetIdentityDigest": digest_domain_json("test", &id).unwrap(),
            "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
            "policyDigest": digest_domain_json("test", &"policy").unwrap(),
            "engram": {
                "projectId": "github.com/unsoldgroup/commonkit",
                "ownerId": owner,
                "projectRoot": "repo",
                "root": "repo/.engram",
                "scope": "project",
                "executable": engram_executable
            }
        })
    };
    let left = sync_config("left", "github:alice", &left_target, "left-adapter");
    let right = sync_config("right", "github:bob", &right_target, "right-adapter");
    let request = serde_json::json!({
        "confirmed": true,
        "confirmationId": "team-reconcile",
        "peerTargetId": "right"
    });
    let config_path = root.join("headless-engram-team.json");
    let mut left_without_executable = left.clone();
    left_without_executable["engram"]
        .as_object_mut()
        .unwrap()
        .remove("executable");
    write_json(
        &config_path,
        &serde_json::json!({"sync": left_without_executable, "syncTargets": [right.clone()]}),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config_path,
            std::sync::Arc::new(PlanStore::open(root.join("plans-missing-executable")).unwrap()),
            root.join("receipts-missing-executable"),
        )
        .is_err()
    );
    write_json(
        &config_path,
        &serde_json::json!({"sync": left, "syncTargets": [right]}),
    );
    let receipt_root = root.join("receipts");
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-missing")).unwrap()),
        receipt_root.clone(),
    )
    .unwrap();

    assert_eq!(
        registry.sync.unwrap().engram_reconcile(request.clone()),
        Err(commonkit_service::DomainFailure::OperationFailed)
    );
    assert!(
        !left_target
            .join("repo/.engram/chunks/22222222.jsonl.gz")
            .exists()
    );
    assert!(
        !right_target
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .exists()
    );
    assert!(!receipt_root.join("engram").exists());

    write_json(
        &config_path,
        &serde_json::json!({
            "sync": sync_config("left", "github:alice", &left_target, "left-adapter"),
            "syncTargets": [sync_config("right", "github:bob", &right_target, "right-adapter")],
            "engramGrants": [{
                "id": "team-grant",
                "projectId": "github.com/unsoldgroup/commonkit",
                "grantor": "github:alice",
                "grantee": "github:bob",
                "state": "withdrawn"
            }]
        }),
    );
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-withdrawn")).unwrap()),
        receipt_root.clone(),
    )
    .unwrap();

    let receipt = registry.sync.unwrap().engram_reconcile(request).unwrap();
    assert_eq!(receipt["importsCompleted"], false);
    assert_eq!(
        receipt["reconciliation"]["unresolved"],
        "grant withdrawn; previously materialized observations are retained"
    );
    assert!(
        receipt["reconciliation"]["moved"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !left_target
            .join("repo/.engram/chunks/22222222.jsonl.gz")
            .exists()
    );
    assert!(
        !right_target
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .exists()
    );
    assert_eq!(
        std::fs::read_dir(receipt_root.join("engram"))
            .unwrap()
            .count(),
        1
    );

    write_engram_project_attestation(&left_target.join("repo/.engram"), &[("11111111", b"left")]);
    write_engram_project_attestation(
        &right_target.join("repo/.engram"),
        &[("22222222", b"right")],
    );
    write_json(
        &config_path,
        &serde_json::json!({
            "sync": sync_config("left", "github:alice", &left_target, "left-adapter"),
            "syncTargets": [sync_config("right", "github:bob", &right_target, "right-adapter")],
            "engramGrants": [
                {
                    "id": "team-grant-active",
                    "projectId": "github.com/unsoldgroup/commonkit",
                    "grantor": "github:alice",
                    "grantee": "github:bob",
                    "state": "active"
                },
                {
                    "id": "team-grant-withdrawn",
                    "projectId": "github.com/unsoldgroup/commonkit",
                    "grantor": "github:bob",
                    "grantee": "github:alice",
                    "state": "withdrawn"
                }
            ]
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config_path,
            std::sync::Arc::new(PlanStore::open(root.join("plans-conflicting-grants")).unwrap()),
            root.join("receipts-conflicting-grants"),
        )
        .is_err()
    );
    write_json(
        &config_path,
        &serde_json::json!({
            "sync": sync_config("left", "github:alice", &left_target, "left-adapter"),
            "syncTargets": [sync_config("right", "github:bob", &right_target, "right-adapter")],
            "engramGrants": [{
                "id": "team-grant",
                "projectId": "github.com/unsoldgroup/commonkit",
                "grantor": "github:alice",
                "grantee": "github:bob",
                "state": "active"
            }]
        }),
    );
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-active")).unwrap()),
        receipt_root.clone(),
    )
    .unwrap();

    let receipt = registry
        .sync
        .unwrap()
        .engram_reconcile(serde_json::json!({
            "confirmed": true,
            "confirmationId": "team-reconcile-active",
            "peerTargetId": "right"
        }))
        .unwrap();
    assert_eq!(receipt["importsCompleted"], true);
    assert_eq!(
        receipt["reconciliation"]["moved"].as_array().unwrap().len(),
        2
    );
    assert!(
        left_target
            .join("repo/.engram/chunks/22222222.jsonl.gz")
            .is_file()
    );
    assert!(
        right_target
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .is_file()
    );
    assert_eq!(
        std::fs::read_dir(receipt_root.join("engram"))
            .unwrap()
            .count(),
        2
    );
}

fn lifecycle_config() -> serde_json::Value {
    #[cfg(windows)]
    let (executable, args) = (
        std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32/cmd.exe"),
        serde_json::json!(["/C", "exit", "0"]),
    );
    #[cfg(not(windows))]
    let (executable, args) = (
        std::path::PathBuf::from("/usr/bin/true"),
        serde_json::json!([]),
    );
    serde_json::json!({
        "stop": {"executable": executable, "args": args},
        "start": {"executable": executable, "args": args}
    })
}

fn file_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn styleguide_descriptor(
    retention: char,
    manifest_digest: &str,
    lock_digest: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 1,
        "id": "commonkit-technical-writing",
        "exportedSkill": "technical-writing",
        "modes": ["strict", "technical"],
        "proseScopes": ["documentation"],
        "exclusions": ["source-code"],
        "evaluationSuite": "technical-writing-v1",
        "evaluationSuiteDigest": format!("sha256:{}", "1".repeat(64)),
        "upstreamUrl": "https://example.com/upstream",
        "upstreamRevision": "b912d5fa59f368253683af2ebfac64ad6d08312d",
        "retentionMapDigest": format!("sha256:{}", retention.to_string().repeat(64)),
        "package": {
            "id": "commonkit-styleguide",
            "version": "0.1.0",
            "manifestDigest": manifest_digest,
            "lockDigest": lock_digest
        }
    })
}

#[test]
fn styleguide_binding_participates_in_plans_and_invalidates_existing_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let target = root.join("target");
    let state = root.join("state");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&state).unwrap();

    let artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    let skill = artifacts
        .put(
            b"---\nname: technical-writing\n---\n",
            ContentSensitivity::Portable,
        )
        .unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("0.1.0").unwrap(),
        "commonkit.native-provider.v1".into(),
        BTreeMap::from([(
            "manifest".into(),
            digest_domain_json("test.styleguide", &"manifest").unwrap(),
        )]),
        vec!["skills".into()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse(
                    "agent-context/.agents/skills/technical-writing/SKILL.md",
                )
                .unwrap(),
                content: skill,
                mode: None,
                expected_before: None,
            }
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "providers/apm/.apm/skills/technical-writing/SKILL.md".into(),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let materialized_path = root.join("apm.json");
    write_json(&materialized_path, &materialized);
    let manifest = b"name: commonkit-styleguide\n";
    let lockfile = b"lockfile_version: \"1\"\n";
    let manifest_path = root.join("apm.yml");
    let lockfile_path = root.join("apm.lock.yaml");
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::write(&lockfile_path, lockfile).unwrap();
    let descriptor_path = root.join("styleguide-descriptor.json");
    write_json(
        &descriptor_path,
        &styleguide_descriptor('2', &file_digest(manifest), &file_digest(lockfile)),
    );
    let config_path = root.join("headless.json");
    write_json(
        &config_path,
        &serde_json::json!({
            "sync": {
                "targetId": "local",
                "targetRoot": target,
                "adapterState": state.join("filesystem"),
                "providerArtifacts": root.join("provider-artifacts"),
                "materializedStates": [materialized_path],
                "styleguide": {
                    "selection": {"skillId": "technical-writing", "activation": "routed"},
                    "descriptors": [descriptor_path],
                    "manifest": manifest_path,
                    "lockfile": lockfile_path,
                    "policy": {"denied": false, "pinnedSkillId": "technical-writing"}
                },
                "declaredRoots": ["agent-context"],
                "protectedRoots": [],
                "caseSensitive": true,
                "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
                "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
                "policyDigest": digest_domain_json("test", &"policy").unwrap()
            }
        }),
    );
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let sync = registry.sync.unwrap();
    let initial: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({
            "confirmed": true,
            "confirmationId": "styleguide-initial"
        }))
        .unwrap(),
    )
    .unwrap();
    sync.plan_execution_authority(&initial).unwrap();

    write_json(
        &descriptor_path,
        &styleguide_descriptor('5', &file_digest(manifest), &file_digest(lockfile)),
    );
    assert_eq!(
        sync.plan_execution_authority(&initial),
        Err(commonkit_service::DomainFailure::OperationFailed)
    );
    let changed: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({
            "confirmed": true,
            "confirmationId": "styleguide-changed"
        }))
        .unwrap(),
    )
    .unwrap();
    assert_ne!(
        initial.bindings.provider_inputs_digest,
        changed.bindings.provider_inputs_digest
    );
}

#[test]
fn configured_registry_materializes_real_plans_credentials_and_snapshots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let target = root.join("target");
    let state = root.join("state");
    let provider_artifacts = ArtifactStore::open(root.join("provider-artifacts")).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let content = provider_artifacts
        .put(b"managed\n", ContentSensitivity::Portable)
        .unwrap();
    let input_digest = digest_domain_json("test.input", &"native").unwrap();
    let inputs = ProviderInputs::new(
        StableId::parse("native").unwrap(),
        ExactProviderVersion::parse("1.0.0").unwrap(),
        "1".into(),
        BTreeMap::from([("manifest".into(), input_digest)]),
        vec!["files".into()],
    )
    .unwrap();
    let materialized = MaterializedState::finalize(
        inputs.clone(),
        vec![NormalizedResource {
            intent: FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/config.txt").unwrap(),
                content: content.clone(),
                mode: None,
                expected_before: None,
            }
            .into(),
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "fixture".into(),
            },
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let materialized_path = root.join("materialized.json");
    write_json(&materialized_path, &materialized);
    let source_secret = root.join("source.secret");
    std::fs::write(&source_secret, b"never serialize me").unwrap();
    let database = root.join("context.sqlite");
    std::fs::write(&database, b"database-v1").unwrap();
    std::fs::create_dir_all(target.join("repo/.engram/chunks")).unwrap();
    std::fs::write(
        target.join("repo/.engram/chunks/08adcd96.jsonl.gz"),
        b"opaque-compressed-chunk",
    )
    .unwrap();
    write_json(
        &target.join("repo/.engram/manifest.json"),
        &serde_json::json!({
            "version": 1,
            "chunks": [{
                "id": "08adcd96", "created_by": "engram", "created_at": "2026-08-06T00:00:00Z",
                "sessions": 0, "memories": 1, "prompts": 0
            }]
        }),
    );
    let git_authority = snapshot_git_authority(root);
    let config = serde_json::json!({
      "sync": {
        "targetId": "local", "targetRoot": target, "adapterState": state.join("filesystem"),
        "providerArtifacts": root.join("provider-artifacts"), "materializedStates": [materialized_path],
        "declaredRoots": ["home"], "protectedRoots": [], "caseSensitive": true,
        "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
        "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
        "policyDigest": digest_domain_json("test", &"policy").unwrap()
        ,"engram": {
          "projectId": "github.com/unsoldgroup/commonkit", "ownerId": "owner-al",
          "projectRoot": "repo", "root": "repo/.engram", "scope": "project",
          "executable": if cfg!(windows) {
            std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32/cmd.exe")
          } else {
            std::path::PathBuf::from("/usr/bin/true")
          }
        }
      },
      "credentials": { "root": root.join("credentials"), "destinations": [{
        "id": "api-token", "reference": format!("file://{}", source_secret.display()), "path": "tokens/api"
      }]},
      "snapshots": { "root": root.join("snapshots"), "portableState": root.join("kit"), "keyReference": format!("file://{}", source_secret.display()),
        "objectStore": {"type":"local"},
        "gitAuthority": git_authority,
        "databases": [{"id":"context-mode", "path": database, "targetId":"local", "observedPaths":{"workstation-b":root.join("context-candidate.db")}, "format":"file", "lifecycle": lifecycle_config()}] }
    });
    let config_path = root.join("headless.json");
    write_json(&config_path, &config);
    let plan_store = std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap());
    let registry =
        ProductionDomainRegistry::load(&config_path, plan_store, root.join("receipts")).unwrap();

    let sync = registry.sync.as_ref().unwrap().clone();
    assert_eq!(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test","unexpected":true})),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );
    let plan = sync
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
        .unwrap();
    assert_eq!(plan["targetId"], "local");
    assert_eq!(
        plan["operations"][0]["resource"]["managedPath"],
        "home/config.txt"
    );
    let plan_contract: commonkit_contracts::Plan = serde_json::from_value(plan).unwrap();
    sync.plan_execution_authority(&plan_contract)
        .expect("authenticated exact plan authority");
    let authority_path = state.join("filesystem/provider-authority").join(format!(
        "{}.json",
        plan_contract.id.as_str().trim_start_matches("sha256:")
    ));
    let authority_bytes = std::fs::read(&authority_path).unwrap();
    let mut tampered: serde_json::Value = serde_json::from_slice(&authority_bytes).unwrap();
    let first = tampered["ciphertext"][0].as_u64().unwrap();
    tampered["ciphertext"][0] = serde_json::json!(first ^ 1);
    write_json(&authority_path, &tampered);
    assert_eq!(
        sync.plan_execution_authority(&plan_contract),
        Err(commonkit_service::DomainFailure::OperationFailed),
        "an unauthenticated authority rewrite must fail before execution"
    );
    std::fs::write(&authority_path, authority_bytes).unwrap();
    let replayed_plan = commonkit_core::build_plan(commonkit_core::PlanDraft {
        target_id: plan_contract.target_id.clone(),
        desired_digest: digest_domain_json("test", &"replayed-desired").unwrap(),
        observed_digest: plan_contract.observed_digest.clone(),
        policy_digest: plan_contract.policy_digest.clone(),
        bindings: plan_contract.bindings.clone(),
        operations: plan_contract.operations.clone(),
    })
    .unwrap();
    let replay_path = state.join("filesystem/provider-authority").join(format!(
        "{}.json",
        replayed_plan.id.as_str().trim_start_matches("sha256:")
    ));
    std::fs::copy(&authority_path, replay_path).unwrap();
    assert_eq!(
        sync.plan_execution_authority(&replayed_plan),
        Err(commonkit_service::DomainFailure::OperationFailed),
        "an authenticated envelope cannot be replayed under another plan"
    );

    let unavailable_artifacts = root.join("provider-artifacts-unavailable");
    std::fs::rename(root.join("provider-artifacts"), &unavailable_artifacts).unwrap();
    let before_rejected_apply = filesystem_snapshot(root);
    let control = ControlPlane::new(std::sync::Arc::new(UnexpectedExecutor));
    control
        .set_target_sync_domains(BTreeMap::from([(
            plan_contract.target_id.clone(),
            sync.clone() as std::sync::Arc<dyn SyncDomain>,
        )]))
        .unwrap();
    let registered = control.register_plan(plan_contract.clone()).unwrap();
    assert!(matches!(
        control.apply(
            &registered.id,
            &StableId::parse("read-only-authority-check").unwrap(),
            "missing-artifact-root",
        ),
        Err(ControlError::PlanAuthorityChanged)
    ));
    assert!(!root.join("provider-artifacts").exists());
    assert_eq!(
        filesystem_snapshot(root),
        before_rejected_apply,
        "rejected apply must not create, chmod, repair, or otherwise mutate durable state"
    );
    std::fs::rename(unavailable_artifacts, root.join("provider-artifacts")).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let artifact_root = root.join("provider-artifacts");
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let before_rejected_apply = filesystem_snapshot(root);
        assert!(matches!(
            control.apply(
                &registered.id,
                &StableId::parse("read-only-permission-check").unwrap(),
                "unsafe-artifact-root-mode",
            ),
            Err(ControlError::PlanAuthorityChanged)
        ));
        assert_eq!(
            filesystem_snapshot(root),
            before_rejected_apply,
            "rejected apply must not repair unsafe artifact-store permissions"
        );
        assert_eq!(
            std::fs::symlink_metadata(&artifact_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700)).unwrap();

        let blob = std::fs::read_dir(&artifact_root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "blob")
            })
            .unwrap();
        let original_blob = blob.with_extension("original-blob");
        let attacker_blob = root.join("attacker-controlled-blob");
        std::fs::rename(&blob, &original_blob).unwrap();
        std::fs::write(&attacker_blob, b"managed\n").unwrap();
        symlink(&attacker_blob, &blob).unwrap();
        let before_rejected_apply = filesystem_snapshot(root);
        assert!(matches!(
            control.apply(
                &registered.id,
                &StableId::parse("artifact-symlink-check").unwrap(),
                "artifact-symlink-substitution",
            ),
            Err(ControlError::PlanAuthorityChanged)
        ));
        assert_eq!(
            filesystem_snapshot(root),
            before_rejected_apply,
            "rejected apply must not mutate any root or entry metadata"
        );
        std::fs::remove_file(&blob).unwrap();
        std::fs::rename(original_blob, blob).unwrap();
        std::fs::remove_file(attacker_blob).unwrap();
    }

    let receipts = ReceiptStore::open(root.join("receipts")).unwrap();
    let run_id = StableId::parse("production-run").unwrap();
    let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(&plan_contract, run_id.clone(), &mut adapters)
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    assert_eq!(
        std::fs::read(target.join("home/config.txt")).unwrap(),
        b"managed\n"
    );
    let verified = sync
        .verify(serde_json::json!({"planId": plan_contract.id}))
        .unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["engram"]["state"], "in_sync");
    assert_eq!(
        verified["engram"]["projectId"],
        "github.com/unsoldgroup/commonkit"
    );
    assert_eq!(verified["engram"]["present"][0]["id"], "08adcd96");

    let converged_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({
            "confirmed": true,
            "confirmationId": "converged-production-plan"
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        converged_plan.operations.is_empty(),
        "applying the desired state must make the next production plan a no-op"
    );

    std::fs::write(target.join("home/config.txt"), b"hand edit").unwrap();
    assert_eq!(
        sync.verify(serde_json::json!({"planId": plan_contract.id})),
        Err(commonkit_service::DomainFailure::VerificationFailed),
        "verification must report an explicit parity failure after a managed-file hand edit"
    );
    std::fs::write(target.join("home/config.txt"), b"managed\n").unwrap();

    let rolled_back = sync
        .rollback(serde_json::json!({
            "confirmed":true,
            "confirmationId":"test",
            "runId":run_id,
            "planId":plan_contract.id
        }))
        .unwrap();
    assert_eq!(rolled_back["outcome"], "rolledback");
    assert!(!target.join("home/config.txt").exists());

    let stale_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(target.join("home")).unwrap();
    std::fs::write(target.join("home/config.txt"), b"hand edit").unwrap();
    let rebound_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    assert_ne!(stale_plan.observed_digest, rebound_plan.observed_digest);
    let mut stale_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(
                &stale_plan,
                StableId::parse("stale-production-run").unwrap(),
                &mut stale_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Canceled
    );
    std::fs::remove_file(target.join("home/config.txt")).unwrap();

    let artifact_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    let artifact_path = state.join("filesystem/artifacts").join(format!(
        "{}.blob",
        content.digest.as_str().trim_start_matches("sha256:")
    ));
    std::fs::write(&artifact_path, b"tampered").unwrap();
    let mut artifact_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_ne!(
        Reconciler::with_store(&receipts)
            .execute(
                &artifact_plan,
                StableId::parse("tampered-artifact-run").unwrap(),
                &mut artifact_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    assert!(!target.join("home/config.txt").exists());
    std::fs::write(&artifact_path, b"managed\n").unwrap();

    let authenticated_plan: commonkit_contracts::Plan = serde_json::from_value(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"test"}))
            .unwrap(),
    )
    .unwrap();
    let authenticated_run = StableId::parse("authenticated-production-run").unwrap();
    let mut authenticated_adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
        commonkit_adapters::FileAdapter::open(&target, &state.join("filesystem")).unwrap(),
    )];
    assert_eq!(
        Reconciler::with_store(&receipts)
            .execute(
                &authenticated_plan,
                authenticated_run.clone(),
                &mut authenticated_adapters,
            )
            .unwrap(),
        ReconcileOutcome::Succeeded
    );
    let receipt_directory = root.join("receipts").join(authenticated_run.as_str());
    let latest_receipt = std::fs::read_dir(receipt_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .max()
        .unwrap();
    std::fs::write(latest_receipt, b"{}").unwrap();
    assert_eq!(
        sync.rollback(serde_json::json!({
            "confirmed":true,
            "confirmationId":"test",
            "runId":authenticated_run,
            "planId":authenticated_plan.id
        })),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );

    let credentials = registry.credentials.unwrap();
    let credential_plan = credentials
        .plan(serde_json::json!({"destinationIds":["api-token"]}))
        .unwrap();
    let applied = credentials
        .apply(serde_json::json!({
            "confirmed": true,
            "confirmationId": "test",
            "planId": credential_plan["planId"]
        }))
        .unwrap();
    assert_eq!(applied["applied"], serde_json::json!(["api-token"]));
    assert_eq!(applied["planId"], credential_plan["planId"]);
    assert!(
        applied["receiptId"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(
        std::fs::read(root.join("credentials/tokens/api")).unwrap(),
        b"never serialize me"
    );
    assert!(!applied.to_string().contains("never serialize me"));

    let snapshots = registry.snapshots.unwrap();
    let created = snapshots.create(serde_json::json!({"confirmed":true,"confirmationId":"test","databaseId":"context-mode"})).unwrap();
    assert_eq!(created["databaseId"], "context-mode");
    assert_eq!(
        snapshots.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let snapshot_listing = snapshots.list().unwrap().to_string();
    assert!(!snapshot_listing.contains("never serialize me"));
    let encrypted_objects = std::fs::read_dir(root.join("snapshots/objects"))
        .unwrap()
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(encrypted_objects.len(), 2);
    assert!(encrypted_objects.iter().all(|object| {
        !object
            .windows(b"context-mode".len())
            .any(|window| window == b"context-mode")
            && !object
                .windows(b"never serialize me".len())
                .any(|window| window == b"never serialize me")
    }));
    std::fs::write(&database, b"database-v2").unwrap();
    snapshots.restore(serde_json::json!({"confirmed":true,"confirmationId":"test","snapshotId":created["snapshotId"]})).unwrap();
    assert_eq!(std::fs::read(database).unwrap(), b"database-v1");
    std::fs::write(root.join("context-candidate.db"), b"database-v1").unwrap();
    snapshots.promote(serde_json::json!({"confirmed":true,"confirmationId":"test","databaseId":"context-mode","targetId":"workstation-b"})).unwrap();
    assert_eq!(
        snapshots.list().unwrap()["writers"]["context-mode"],
        "workstation-b"
    );
}

#[test]
fn snapshot_promotion_rejects_unsnapshotted_authoritative_changes_and_missing_observations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let database = root.join("writer.db");
    let candidate = root.join("candidate.db");
    std::fs::write(&database, b"snapshot-state").unwrap();
    std::fs::write(&candidate, b"snapshot-state").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let git_authority = snapshot_git_authority(root);
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "gitAuthority": git_authority,
                "databases": [{
                    "id":"context-mode",
                    "path": database,
                    "targetId":"writer",
                    "observedPaths":{"candidate":candidate},
                    "format":"file",
                    "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    let registry = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();
    let snapshots = registry.snapshots.unwrap();
    assert_eq!(
        snapshots.list().unwrap()["promotionEvidence"]["context-mode"]["observableTargets"],
        serde_json::json!(["candidate", "writer"])
    );
    snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    std::fs::write(&database, b"newer-authoritative-state").unwrap();

    assert_eq!(
        snapshots.promote(serde_json::json!({"databaseId":"context-mode","targetId":"candidate"})),
        Err(commonkit_service::DomainFailure::VerificationFailed)
    );
    assert_eq!(
        snapshots.list().unwrap()["writers"]["context-mode"],
        "writer"
    );
    assert_eq!(
        snapshots.promote(serde_json::json!({
            "databaseId":"context-mode",
            "targetId":"unconfigured",
            "currentWriterDigest":"sha256:caller-assertion",
            "candidateDigest":"sha256:caller-assertion"
        })),
        Err(commonkit_service::DomainFailure::InvalidRequest)
    );
}

#[test]
fn promoted_writer_is_the_only_source_allowed_to_advance_snapshot_history() {
    use sha2::{Digest, Sha256};

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let writer_a = root.join("writer-a.db");
    let writer_b = root.join("writer-b.db");
    std::fs::write(&writer_a, b"state-v1").unwrap();
    std::fs::write(&writer_b, b"state-v1").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let git_authority = snapshot_git_authority(root);
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "gitAuthority": git_authority,
                "databases": [{
                    "id":"context-mode",
                    "path":writer_a,
                    "targetId":"machine-a",
                    "observedPaths":{"machine-b":writer_b},
                    "format":"file",
                    "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    let snapshots = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap()
    .snapshots
    .unwrap();

    let first = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    snapshots
        .promote(serde_json::json!({
            "databaseId":"context-mode",
            "targetId":"machine-b",
            "confirmationId":"promote-to-b"
        }))
        .unwrap();
    std::fs::write(&writer_b, b"state-v2-from-b").unwrap();
    let second = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();

    let listing = snapshots.list().unwrap();
    let manifests = listing["snapshots"].as_array().unwrap();
    let second_manifest = manifests
        .iter()
        .find(|item| item["snapshotId"] == second["snapshotId"])
        .unwrap();
    assert_eq!(second_manifest["manifest"]["sourceTarget"], "machine-b");
    assert_eq!(
        second_manifest["manifest"]["parentDigest"],
        format!("sha256:{:x}", Sha256::digest(b"state-v1"))
    );

    // A stale former writer cannot create a new branch or become authoritative again merely by
    // changing its local bytes. With B unchanged, create is a no-op at the true chain head.
    std::fs::write(&writer_a, b"state-v3-from-old-a").unwrap();
    let after_old_writer_change = snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    assert_eq!(after_old_writer_change["snapshotId"], second["snapshotId"]);
    assert_ne!(first["snapshotId"], second["snapshotId"]);
    assert_eq!(
        snapshots.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn fresh_clone_rejects_deleted_anchors_and_force_rolled_back_snapshot_authority_branch() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let database = root.join("writer.db");
    std::fs::write(&database, b"state-v1").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let git_authority = snapshot_git_authority(root);
    let remote = root.join(".snapshot-authority-remote.git");
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "gitAuthority": git_authority,
                "databases": [{
                    "id":"context-mode", "path":database, "targetId":"writer",
                    "observedPaths":{}, "format":"file", "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    let snapshots = ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap()
    .snapshots
    .unwrap();
    snapshots
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();

    let reference = "refs/commonkit-authority/context-mode";
    let status = std::process::Command::new(git_executable())
        .arg("-C")
        .arg(root)
        .args(["push", remote.to_str().unwrap(), &format!(":{reference}")])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(root.join("missing-anchor-plans")).unwrap()),
            root.join("missing-anchor-receipts"),
        )
        .is_err(),
        "bootstrap must not recreate deleted anchors for an existing authority record"
    );

    let initial = {
        let output = std::process::Command::new(git_executable())
            .arg("-C")
            .arg(root)
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let status = std::process::Command::new(git_executable())
        .arg("-C")
        .arg(root)
        .args([
            "push",
            "--force",
            remote.to_str().unwrap(),
            &format!("{initial}:refs/heads/main"),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let fresh = root.join("fresh-clone");
    assert!(
        std::process::Command::new(git_executable())
            .args([
                "clone",
                "--branch",
                "main",
                remote.to_str().unwrap(),
                fresh.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );
    let fresh_config = root.join("fresh-headless.json");
    write_json(
        &fresh_config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("fresh-state"),
                "portableState": fresh.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"},
                "gitAuthority": {
                    "executable":git_executable(), "repository":fresh,
                    "trustedRemoteUrl":remote, "branch":"main",
                    "stagingRoot":root.join("fresh-staging")
                },
                "databases": [{
                    "id":"context-mode", "path":database, "targetId":"writer",
                    "observedPaths":{}, "format":"file", "lifecycle":lifecycle_config()
                }]
            }
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &fresh_config,
            std::sync::Arc::new(PlanStore::open(root.join("fresh-plans")).unwrap()),
            root.join("fresh-receipts"),
        )
        .is_err()
    );
}

#[test]
fn production_fresh_clone_accepts_later_unrelated_kit_commits_after_anchor() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let database = root.join("writer.db");
    std::fs::write(&database, b"state-v1").unwrap();
    let key_path = root.join("snapshot-key");
    std::fs::write(&key_path, b"snapshot-test-key").unwrap();
    let git_authority = snapshot_git_authority(root);
    let remote = root.join(".snapshot-authority-remote.git");
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null, "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"), "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key_path.display()),
                "objectStore": {"type":"local"}, "gitAuthority":git_authority,
                "databases":[{"id":"context-mode", "path":database, "targetId":"writer",
                    "observedPaths":{}, "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    ProductionDomainRegistry::load(
        &config,
        std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
        root.join("receipts"),
    )
    .unwrap();

    std::fs::write(root.join("README.md"), b"later unrelated kit commit\n").unwrap();
    for args in [
        vec!["add", "README.md"],
        vec!["commit", "-m", "later unrelated kit commit"],
        vec!["push", ".snapshot-authority-remote.git", "main"],
    ] {
        let output = std::process::Command::new(git_executable())
            .arg("-C")
            .arg(root)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let fresh = root.join("fresh-valid-clone");
    assert!(
        std::process::Command::new(git_executable())
            .args([
                "clone",
                "--branch",
                "main",
                remote.to_str().unwrap(),
                fresh.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );
    let fresh_config = root.join("fresh-valid-headless.json");
    write_json(
        &fresh_config,
        &serde_json::json!({
            "sync":null, "credentials":null,
            "snapshots": {
                "root":root.join("fresh-valid-state"), "portableState":fresh.join("kit"),
                "keyReference":format!("file://{}", key_path.display()),
                "objectStore":{"type":"local"},
                "gitAuthority":{"executable":git_executable(), "repository":fresh,
                    "trustedRemoteUrl":remote, "branch":"main",
                    "stagingRoot":root.join("fresh-valid-staging")},
                "databases":[{"id":"context-mode", "path":database, "targetId":"writer",
                    "observedPaths":{}, "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    ProductionDomainRegistry::load(
        &fresh_config,
        std::sync::Arc::new(PlanStore::open(root.join("fresh-valid-plans")).unwrap()),
        root.join("fresh-valid-receipts"),
    )
    .unwrap();
}

#[test]
fn production_startup_completes_promotion_after_each_post_push_interruption_window() {
    use sha2::{Digest, Sha256};

    for (index, failpoint) in [
        PublisherFailpoint::AfterPushBeforeLocalRef,
        PublisherFailpoint::AfterLocalRefBeforePortableInstall,
    ]
    .into_iter()
    .enumerate()
    {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let database_path = root.join("writer.db");
        let candidate_path = root.join("candidate.db");
        std::fs::write(&database_path, b"state-v1").unwrap();
        std::fs::write(&candidate_path, b"state-v1").unwrap();
        let secret = b"snapshot-test-key";
        let key_path = root.join("snapshot-key");
        std::fs::write(&key_path, secret).unwrap();
        let git_authority = snapshot_git_authority(root);
        let remote = root.join(".snapshot-authority-remote.git");
        let config = root.join("headless.json");
        write_json(
            &config,
            &serde_json::json!({
                "sync": null, "credentials": null,
                "snapshots": {
                    "root":root.join("snapshots"), "portableState":root.join("kit"),
                    "keyReference":format!("file://{}", key_path.display()),
                    "objectStore":{"type":"local"}, "gitAuthority":git_authority,
                    "databases":[{
                        "id":"context-mode", "path":database_path, "targetId":"writer",
                        "observedPaths":{"candidate":candidate_path}, "format":"file",
                        "lifecycle":lifecycle_config()
                    }]
                }
            }),
        );
        let registry = ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
            root.join("receipts"),
        )
        .unwrap();
        registry
            .snapshots
            .as_ref()
            .unwrap()
            .create(serde_json::json!({"databaseId":"context-mode"}))
            .unwrap();

        let key: [u8; 32] = Sha256::digest(secret).into();
        let cipher = XChaCha20Cipher::new(key);
        let database = DatabaseId::new("context-mode").unwrap();
        let portable = PortableAuthorityStore::open_with_trusted_anchor(
            root.join("kit"),
            root.join("snapshots/portable-authority-anchors"),
            &cipher,
        )
        .unwrap();
        let current = portable.read(&database).unwrap();
        let content_digest = format!("sha256:{:x}", Sha256::digest(b"state-v1"));
        let run_id = format!("interrupted-promotion-{index}");
        let local = AuthorityStore::open(root.join("snapshots/authority")).unwrap();
        local
            .prepare_promotion(PromotionPlan {
                schema: "commonkit.promotion-plan.v1".into(),
                run_id: run_id.clone(),
                database: database.clone(),
                previous_writer: "writer".into(),
                candidate_writer: "candidate".into(),
                latest_snapshot_digest: content_digest.clone(),
                current_writer_digest: content_digest.clone(),
                candidate_digest: content_digest,
            })
            .unwrap();
        let mut publisher = ProcessGitAuthorityPublisher::new(
            git_executable(),
            root,
            remote.to_str().unwrap(),
            "main",
            "kit",
            root.join("snapshot-git-staging"),
        )
        .unwrap();
        let parent = publisher.trusted_remote_revision().unwrap();
        publisher.set_failpoint(failpoint);
        assert_eq!(
            portable.compare_and_swap_writer_published(
                &database,
                &current.revision,
                "writer",
                "candidate",
                &parent,
                &mut publisher,
            ),
            Err(SnapshotError::Interrupted)
        );

        let later_clone = root.join(format!("later-fast-forward-{index}"));
        assert!(
            std::process::Command::new(git_executable())
                .args([
                    "clone",
                    "--branch",
                    "main",
                    remote.to_str().unwrap(),
                    later_clone.to_str().unwrap(),
                ])
                .status()
                .unwrap()
                .success()
        );
        for args in [
            ["config", "user.name", "Unrelated Writer"],
            ["config", "user.email", "unrelated-writer@localhost"],
        ] {
            let status = std::process::Command::new(git_executable())
                .arg("-C")
                .arg(&later_clone)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let unrelated = format!("unrelated-after-interruption-{index}.txt");
        std::fs::write(later_clone.join(&unrelated), b"retain me").unwrap();
        for args in [
            vec!["add", unrelated.as_str()],
            vec!["commit", "-m", "unrelated fast-forward"],
            vec!["push", "origin", "main"],
        ] {
            let status = std::process::Command::new(git_executable())
                .arg("-C")
                .arg(&later_clone)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }

        drop(registry);
        let restarted = ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(root.join("plans-restarted")).unwrap()),
            root.join("receipts-restarted"),
        )
        .unwrap();
        assert_eq!(
            restarted.snapshots.unwrap().list().unwrap()["writers"]["context-mode"],
            "candidate"
        );
        assert!(local.unfinished_promotions().unwrap().is_empty());
        assert_eq!(std::fs::read(root.join(unrelated)).unwrap(), b"retain me");
        assert!(
            root.join("snapshots/authority/promotions")
                .join(run_id)
                .join("receipt.json")
                .is_file()
        );
    }
}

#[test]
fn portable_descriptor_discovers_and_restores_snapshot_on_another_machine() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let portable_state = root.join("kit");
    let key = root.join("provisioned-snapshot-key");
    std::fs::write(&key, b"same-key-provisioned-outside-git").unwrap();
    let source_database = root.join("machine-a.db");
    std::fs::write(&source_database, b"portable database plaintext").unwrap();

    let config_a = root.join("machine-a.json");
    write_json(
        &config_a,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("machine-a-state"),
                "portableState": portable_state,
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":source_database, "targetId":"machine-a", "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    let snapshots_a = ProductionDomainRegistry::load(
        &config_a,
        std::sync::Arc::new(PlanStore::open(root.join("plans-a")).unwrap()),
        root.join("receipts-a"),
    )
    .unwrap()
    .snapshots
    .unwrap();
    let created = snapshots_a
        .create(serde_json::json!({"databaseId":"context-mode"}))
        .unwrap();
    drop(snapshots_a);

    // A shared S3 backend supplies these immutable ciphertext objects in production. Copying the
    // local test backend models the same object availability without putting database bytes in Git.
    let machine_b_state = root.join("machine-b-state");
    std::fs::create_dir_all(machine_b_state.join("objects")).unwrap();
    for entry in std::fs::read_dir(root.join("machine-a-state/objects")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            machine_b_state.join("objects").join(entry.file_name()),
        )
        .unwrap();
    }
    let candidate_database = root.join("machine-b.db");
    std::fs::write(&candidate_database, b"stale local database").unwrap();
    let config_b = root.join("machine-b.json");
    write_json(
        &config_b,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": machine_b_state,
                "portableState": portable_state,
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"local"},
                "databases": [{"id":"context-mode", "path":candidate_database, "targetId":"machine-b", "format":"file", "lifecycle":lifecycle_config()}]
            }
        }),
    );
    let snapshots_b = ProductionDomainRegistry::load(
        &config_b,
        std::sync::Arc::new(PlanStore::open(root.join("plans-b")).unwrap()),
        root.join("receipts-b"),
    )
    .unwrap()
    .snapshots
    .unwrap();
    assert_eq!(
        snapshots_b.list().unwrap()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    snapshots_b
        .restore(serde_json::json!({"snapshotId":created["snapshotId"]}))
        .unwrap();
    assert_eq!(
        std::fs::read(candidate_database).unwrap(),
        b"portable database plaintext"
    );
    for entry in std::fs::read_dir(root.join("kit/snapshots")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(b"portable database plaintext".len())
                .any(|window| { window == b"portable database plaintext" })
        );
        assert!(
            !bytes
                .windows(b"same-key-provisioned-outside-git".len())
                .any(|window| { window == b"same-key-provisioned-outside-git" })
        );
    }
}

#[test]
fn configured_git_provider_pipeline_materializes_native_state_before_local_planning() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let checkout = root.join("checkout");
    git(root, &["init", "--bare", remote.to_str().unwrap()]);
    git(root, &["init", seed.to_str().unwrap()]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "CommonKit Test"]);
    std::fs::create_dir_all(seed.join("portable")).unwrap();
    std::fs::write(seed.join("portable/editor.conf"), b"from pinned git\n").unwrap();
    git(&seed, &["add", "portable/editor.conf"]);
    git(&seed, &["commit", "-m", "fixture"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);
    git(
        root,
        &[
            "clone",
            remote.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git(&checkout, &["checkout", "main"]);
    let revision = git_output(&checkout, &["rev-parse", "HEAD"]);
    let target = root.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let config = serde_json::json!({
      "sync": {
        "targetId": "local", "targetRoot": target,
        "adapterState": root.join("adapter"),
        "providerArtifacts": root.join("artifacts"),
        "providerPipeline": {
          "root": root.join("pipeline"),
          "source": {"repository": checkout, "trustedRemoteUrl": remote.to_str().unwrap(), "revision": revision},
          "providers": [{"provider":"native", "version":"1.0.0", "files":[{
            "path":"home/editor.conf", "source":"portable/editor.conf"
          }]}]
        },
        "declaredRoots": ["home"], "protectedRoots": [], "caseSensitive": true,
        "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
        "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
        "policyDigest": digest_domain_json("test", &"policy").unwrap()
      }
    });
    let config_path = root.join("headless-provider.json");
    write_json(&config_path, &config);
    let registry = ProductionDomainRegistry::load(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-pipeline")).unwrap()),
        root.join("receipts-pipeline"),
    )
    .unwrap();
    let sync = registry.sync.unwrap();
    let fetched = sync.git_sync(true).unwrap();
    assert_eq!(fetched["state"], "clean");
    assert_eq!(fetched["fetched"], true);
    let plan = sync
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"provider-plan"}))
        .unwrap();
    assert_eq!(plan["operations"].as_array().unwrap().len(), 1);
    assert_eq!(
        plan["operations"][0]["resource"]["managedPath"],
        "home/editor.conf"
    );
    let reviewed: commonkit_contracts::Plan = serde_json::from_value(plan.clone()).unwrap();
    std::fs::rename(checkout.join(".git"), checkout.join(".git-disabled")).unwrap();
    sync.plan_execution_authority(&reviewed)
        .expect("apply authority uses only sealed local facts, never Git inspection");
    std::fs::rename(checkout.join(".git-disabled"), checkout.join(".git")).unwrap();
    let states = std::fs::read_dir(root.join("pipeline/states"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(states.len(), 1);
    std::fs::write(
        root.join("checkout/portable/editor.conf"),
        b"uncommitted drift\n",
    )
    .unwrap();
    assert_eq!(
        sync.plan(serde_json::json!({"confirmed":true,"confirmationId":"stale-source"})),
        Err(commonkit_service::DomainFailure::OperationFailed)
    );
}

#[test]
fn configured_empty_native_provider_produces_a_repeatable_no_op_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let checkout = root.join("checkout");
    git(root, &["init", "--bare", remote.to_str().unwrap()]);
    git(root, &["init", seed.to_str().unwrap()]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "CommonKit Test"]);
    std::fs::write(seed.join("README.md"), b"empty native setup\n").unwrap();
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-m", "fixture"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);
    git(
        root,
        &[
            "clone",
            remote.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git(&checkout, &["checkout", "main"]);
    let revision = git_output(&checkout, &["rev-parse", "HEAD"]);
    let target = root.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let config_path = root.join("headless-provider.json");
    write_json(
        &config_path,
        &serde_json::json!({
          "sync": {
            "targetId": "local", "targetRoot": target,
            "adapterState": root.join("adapter"),
            "providerArtifacts": root.join("artifacts"),
            "providerPipeline": {
              "root": root.join("pipeline"),
              "source": {
                "repository": checkout,
                "trustedRemoteUrl": remote.to_str().unwrap(),
                "revision": revision
              },
              "providers": [{"provider":"native", "version":"1.0.0", "files":[]}]
            },
            "declaredRoots": ["home"], "protectedRoots": [], "caseSensitive": true,
            "targetIdentityDigest": digest_domain_json("test", &"target").unwrap(),
            "composedLoadoutDigest": digest_domain_json("test", &"loadout").unwrap(),
            "policyDigest": digest_domain_json("test", &"policy").unwrap()
          }
        }),
    );
    let registry = ProductionDomainRegistry::load_optional_with_relay_endpoint(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-pipeline")).unwrap()),
        root.join("receipts-pipeline"),
        "127.0.0.1:37641".parse().unwrap(),
    )
    .unwrap();
    let sync = registry.sync.unwrap();
    std::fs::rename(&remote, root.join("remote-unavailable.git")).unwrap();

    let first = sync
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"empty-native-first"}))
        .expect("local planning does not contact the remote without an explicit fetch");
    drop(sync);
    let restarted = ProductionDomainRegistry::load_optional_with_relay_endpoint(
        &config_path,
        std::sync::Arc::new(PlanStore::open(root.join("plans-pipeline")).unwrap()),
        root.join("receipts-pipeline"),
        "127.0.0.1:37642".parse().unwrap(),
    )
    .unwrap();
    let repeated = restarted
        .sync
        .unwrap()
        .plan(serde_json::json!({"confirmed":true,"confirmationId":"empty-native-repeat"}))
        .expect("a relay-port change does not invalidate a plan with no MCP resources");

    assert!(first["operations"].as_array().unwrap().is_empty());
    assert_eq!(repeated["planDigest"], first["planDigest"]);
    assert_eq!(repeated["operations"], first["operations"]);
}

fn git(directory: &std::path::Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(directory: &std::path::Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn absent_configuration_installs_no_fabricated_domains() {
    let temporary = tempfile::tempdir().unwrap();
    let registry = ProductionDomainRegistry::load_optional(
        &temporary.path().join("missing.json"),
        std::sync::Arc::new(PlanStore::open(temporary.path().join("plans")).unwrap()),
        temporary.path().join("receipts"),
    )
    .unwrap();
    assert!(registry.sync.is_none());
    assert!(registry.credentials.is_none());
    assert!(registry.snapshots.is_none());
}

#[test]
fn present_but_incomplete_capability_configuration_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": {"root":"relative", "destinations":[]},
            "snapshots": null
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(temporary.path().join("plans")).unwrap()),
            temporary.path().join("receipts"),
        )
        .is_err()
    );
}

#[test]
fn invalid_s3_snapshot_backend_fails_at_registry_load() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let key = root.join("key");
    std::fs::write(&key, b"target-local-key").unwrap();
    let config = root.join("headless.json");
    write_json(
        &config,
        &serde_json::json!({
            "sync": null,
            "credentials": null,
            "snapshots": {
                "root": root.join("snapshots"),
                "portableState": root.join("kit"),
                "keyReference": format!("file://{}", key.display()),
                "objectStore": {"type":"s3", "executable":"relative/aws", "endpoint":"http://insecure", "bucket":"bad", "prefix":"."},
                "databases": [{"id":"context-mode", "path":root.join("context.sqlite"), "targetId":"local", "format":"sqlite", "lifecycle": lifecycle_config()}]
            }
        }),
    );
    assert!(
        ProductionDomainRegistry::load(
            &config,
            std::sync::Arc::new(PlanStore::open(root.join("plans")).unwrap()),
            root.join("receipts"),
        )
        .is_err()
    );
}

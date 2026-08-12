use std::fs;

use commonkit_adapters::{
    EngramChunkAdapter, EngramChunkSetDeclaration, EngramChunkSetState, EngramCommandOutput,
    EngramCommandRunner, EngramGrant, EngramGrantState, EngramOwnerId, EngramScope,
    EngramTargetChunkSetDeclaration, EngramTargetSyncMode, LocalTargetFilesystem,
    NormalizedManagedPath, ProcessEngramCommandRunner, SshFilesystemRequest, SshFilesystemResponse,
    SshFilesystemTransport, SshTargetFilesystem, TargetFilesystemError,
};
use commonkit_contracts::Principal;
use commonkit_core::{RootAccess, StableId};
use serde_json::json;
use sha2::{Digest, Sha256};

fn write_chunk_set(root: &std::path::Path, chunks: &[(&str, &[u8])]) {
    fs::create_dir_all(root.join("chunks")).unwrap();
    let manifest_chunks = chunks
        .iter()
        .map(|(id, _)| {
            json!({
                "id": id,
                "created_by": "test",
                "created_at": "2026-08-06T00:00:00Z",
                "sessions": 0,
                "memories": 1,
                "prompts": 0
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({"version": 1, "chunks": manifest_chunks})).unwrap(),
    )
    .unwrap();
    for (id, bytes) in chunks {
        fs::write(root.join("chunks").join(format!("{id}.jsonl.gz")), bytes).unwrap();
    }
}

fn write_project_attestation(root: &std::path::Path, chunks: &[(&str, &[u8])]) {
    let manifest = fs::read(root.join("manifest.json")).unwrap();
    let attestation = json!({
        "schema": "engram.scope-export.v1",
        "exporterVersion": "1.17.0",
        "projectId": "github.com/unsoldgroup/commonkit",
        "scope": "project",
        "manifestDigest": format!("sha256:{:x}", Sha256::digest(&manifest)),
        "chunks": chunks.iter().map(|(id, bytes)| json!({
            "id": id,
            "digest": format!("sha256:{:x}", Sha256::digest(bytes)),
            "bytes": bytes.len()
        })).collect::<Vec<_>>()
    });
    fs::write(
        root.join("scope-attestation.json"),
        serde_json::to_vec_pretty(&attestation).unwrap(),
    )
    .unwrap();
}

fn declaration(root: &std::path::Path) -> EngramChunkSetDeclaration {
    EngramChunkSetDeclaration {
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        root: root.to_path_buf(),
        scope: EngramScope::Project,
    }
}

#[test]
fn owner_identity_is_derived_from_the_resolved_principal() {
    let principal = Principal::parse("Alice-Example").unwrap();
    let owner = EngramOwnerId::from_principal(&principal);

    assert_eq!(owner.as_str(), "github:alice-example");
}

#[test]
fn arbitrary_owner_labels_are_rejected() {
    assert!(EngramOwnerId::try_from("local-alias").is_err());
}

#[test]
fn status_reports_missing_and_extra_chunks_without_decoding_payloads() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join(".engram");
    write_chunk_set(
        &root,
        &[("11111111", b"opaque-one"), ("22222222", b"opaque-two")],
    );
    fs::remove_file(root.join("chunks/22222222.jsonl.gz")).unwrap();
    fs::write(root.join("chunks/33333333.jsonl.gz"), b"not-even-gzip").unwrap();

    let state = EngramChunkAdapter::status(&declaration(&root)).unwrap();

    assert_eq!(state.state, EngramChunkSetState::Drifted);
    assert_eq!(state.present.len(), 1);
    assert_eq!(
        state.missing,
        ["22222222"].into_iter().map(String::from).collect()
    );
    assert_eq!(
        state.extra,
        ["33333333"].into_iter().map(String::from).collect()
    );
}

#[test]
fn reconcile_exchanges_append_only_chunks_in_both_directions_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("right/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);
    write_chunk_set(&right, &[("22222222", b"right")]);

    let first = EngramChunkAdapter::reconcile(&declaration(&left), &declaration(&right)).unwrap();
    let second = EngramChunkAdapter::reconcile(&declaration(&left), &declaration(&right)).unwrap();

    assert_eq!(first.moved.len(), 2);
    assert!(first.unresolved.is_none());
    assert!(second.moved.is_empty());
    assert_eq!(
        EngramChunkAdapter::status(&declaration(&left))
            .unwrap()
            .present
            .len(),
        2
    );
    assert_eq!(
        EngramChunkAdapter::status(&declaration(&right))
            .unwrap()
            .present
            .len(),
        2
    );
}

#[test]
fn reconcile_recovers_stale_stage_files_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("right/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);
    write_chunk_set(&right, &[("22222222", b"right")]);
    fs::write(left.join("manifest.json.commonkit-tmp"), b"stale").unwrap();
    fs::write(
        left.join("chunks/33333333.jsonl.gz.commonkit-tmp"),
        b"stale",
    )
    .unwrap();

    let receipt = EngramChunkAdapter::reconcile(&declaration(&left), &declaration(&right)).unwrap();

    assert_eq!(receipt.moved.len(), 2);
    assert!(!left.join("manifest.json.commonkit-tmp").exists());
    assert!(!left.join("chunks/33333333.jsonl.gz.commonkit-tmp").exists());
}

#[cfg(unix)]
#[test]
fn standalone_engram_runner_rejects_a_symlinked_working_directory() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    let link = directory.path().join("working");
    fs::create_dir(&real).unwrap();
    symlink(&real, &link).unwrap();
    let runner = ProcessEngramCommandRunner::try_from_path("/usr/bin/true").unwrap();

    assert!(runner.run(&link, &[]).is_err());
}

#[cfg(unix)]
#[test]
fn standalone_engram_runner_rejects_a_symlinked_working_directory_ancestor() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = directory.path().join("outside");
    let real = outside.join("real");
    let link = directory.path().join("working");
    fs::create_dir_all(&real).unwrap();
    symlink(&outside, &link).unwrap();
    let runner = ProcessEngramCommandRunner::try_from_path("/usr/bin/true").unwrap();

    assert!(runner.run(&link.join("real"), &[]).is_err());
}

#[test]
fn oversized_chunks_fail_before_unbounded_digest_work() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join(".engram");
    write_chunk_set(&root, &[("11111111", &[0_u8; 1024])]);
    fs::write(
        root.join("chunks/11111111.jsonl.gz"),
        vec![0_u8; commonkit_adapters::MAX_ENGRAM_CHUNK_BYTES as usize + 1],
    )
    .unwrap();

    let error = EngramChunkAdapter::status(&declaration(&root)).unwrap_err();

    assert_eq!(error.to_string(), "Engram chunk is too large");
}

#[test]
fn reconcile_refuses_personal_transport_and_project_identity_mismatch() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("right/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);
    write_chunk_set(&right, &[]);
    let mut personal = declaration(&left);
    personal.scope = EngramScope::Personal;

    let personal_error =
        EngramChunkAdapter::reconcile(&personal, &declaration(&right)).unwrap_err();
    let mut other_project = declaration(&right);
    other_project.project_id = "github.com/unsoldgroup/other".try_into().unwrap();
    let identity_error =
        EngramChunkAdapter::reconcile(&declaration(&left), &other_project).unwrap_err();

    assert_eq!(
        personal_error.to_string(),
        "personal Engram chunks are never transportable"
    );
    assert_eq!(
        identity_error.to_string(),
        "Engram project identities do not match"
    );
}

#[test]
fn cross_principal_transport_fails_closed_without_scope_attestation() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("right/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);
    write_chunk_set(&right, &[]);
    let mut teammate = declaration(&right);
    teammate.owner_id = EngramOwnerId::try_from("github:teammate").unwrap();

    let error = EngramChunkAdapter::reconcile(&declaration(&left), &teammate).unwrap_err();

    assert_eq!(
        error.to_string(),
        "cross-principal Engram transport requires project-scope attestation"
    );
}

#[test]
fn unsigned_cross_principal_attestation_is_rejected_even_when_metadata_matches() {
    let directory = tempfile::tempdir().unwrap();
    let left_root = directory.path().join("left-target");
    let right_root = directory.path().join("right-target");
    let left_chunks = [("11111111", b"left".as_slice())];
    let right_chunks = [("22222222", b"right".as_slice())];
    write_chunk_set(&left_root.join("repo/.engram"), &left_chunks);
    write_chunk_set(&right_root.join("repo/.engram"), &right_chunks);
    write_project_attestation(&left_root.join("repo/.engram"), &left_chunks);
    write_project_attestation(&right_root.join("repo/.engram"), &right_chunks);
    let left_fs = LocalTargetFilesystem::open(&left_root, RootAccess::ReadWrite).unwrap();
    let right_fs = LocalTargetFilesystem::open(&right_root, RootAccess::ReadWrite).unwrap();
    let left = EngramTargetChunkSetDeclaration {
        target_id: StableId::parse("alice-mac").unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:alice").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };
    let mut right = left.clone();
    right.target_id = StableId::parse("bob-mac").unwrap();
    right.owner_id = EngramOwnerId::try_from("github:bob").unwrap();
    let grant = EngramGrant {
        id: StableId::parse("commonkit-team-memory").unwrap(),
        project_id: left.project_id.clone(),
        grantor: left.owner_id.clone(),
        grantee: right.owner_id.clone(),
        state: EngramGrantState::Active,
    };

    let error =
        EngramChunkAdapter::reconcile_granted_targets(&left_fs, &left, &right_fs, &right, &grant)
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "cross-principal Engram transport requires project-scope attestation"
    );
}

#[test]
fn withdrawn_grant_is_a_prospective_no_op_and_retains_materialized_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let left_root = directory.path().join("left-target");
    let right_root = directory.path().join("right-target");
    let chunks = [("11111111", b"already-shared".as_slice())];
    write_chunk_set(&left_root.join("repo/.engram"), &chunks);
    write_chunk_set(&right_root.join("repo/.engram"), &chunks);
    write_project_attestation(&left_root.join("repo/.engram"), &chunks);
    write_project_attestation(&right_root.join("repo/.engram"), &chunks);
    let left_fs = LocalTargetFilesystem::open(&left_root, RootAccess::ReadWrite).unwrap();
    let right_fs = LocalTargetFilesystem::open(&right_root, RootAccess::ReadWrite).unwrap();
    let declaration = |target: &str, owner: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from(owner).unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };
    let left = declaration("alice-mac", "github:alice");
    let right = declaration("bob-mac", "github:bob");
    let grant = EngramGrant {
        id: StableId::parse("commonkit-team-memory").unwrap(),
        project_id: left.project_id.clone(),
        grantor: left.owner_id.clone(),
        grantee: right.owner_id.clone(),
        state: EngramGrantState::Withdrawn,
    };

    let receipt =
        EngramChunkAdapter::reconcile_granted_targets(&left_fs, &left, &right_fs, &right, &grant)
            .unwrap();

    assert!(receipt.moved.is_empty());
    assert_eq!(
        receipt.unresolved.as_deref(),
        Some("grant withdrawn; previously materialized observations are retained")
    );
    assert!(
        right_root
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .is_file()
    );
}

#[test]
fn unreachable_peer_is_a_reported_no_op() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("missing/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);

    let receipt = EngramChunkAdapter::reconcile(&declaration(&left), &declaration(&right)).unwrap();

    assert!(receipt.moved.is_empty());
    assert_eq!(
        receipt.unresolved.as_deref(),
        Some("peer chunk set is unreachable")
    );
}

#[derive(Default)]
struct RecordingRunner(std::sync::Mutex<Vec<(std::path::PathBuf, Vec<String>)>>);

impl EngramCommandRunner for RecordingRunner {
    fn run(
        &self,
        working_directory: &std::path::Path,
        arguments: &[&str],
    ) -> Result<EngramCommandOutput, commonkit_adapters::EngramError> {
        self.0.lock().unwrap().push((
            working_directory.to_path_buf(),
            arguments.iter().map(|value| (*value).to_owned()).collect(),
        ));
        Ok(EngramCommandOutput { success: true })
    }
}

#[test]
fn installed_reconcile_exports_then_imports_without_manual_engram_steps() {
    let directory = tempfile::tempdir().unwrap();
    let left = directory.path().join("left/.engram");
    let right = directory.path().join("right/.engram");
    write_chunk_set(&left, &[("11111111", b"left")]);
    write_chunk_set(&right, &[]);
    let runner = RecordingRunner::default();

    let receipt =
        EngramChunkAdapter::reconcile_installed(&runner, &declaration(&left), &declaration(&right))
            .unwrap();

    assert_eq!(receipt.moved.len(), 1);
    assert_eq!(
        runner.0.into_inner().unwrap(),
        vec![
            (
                directory.path().join("left"),
                vec!["sync", "--project", "github.com/unsoldgroup/commonkit"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            ),
            (
                directory.path().join("right"),
                vec!["sync", "--project", "github.com/unsoldgroup/commonkit"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            ),
            (
                directory.path().join("left"),
                vec![
                    "sync",
                    "--import",
                    "--project",
                    "github.com/unsoldgroup/commonkit"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            ),
            (
                directory.path().join("right"),
                vec![
                    "sync",
                    "--import",
                    "--project",
                    "github.com/unsoldgroup/commonkit"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            ),
        ]
    );
}

#[test]
fn target_reconcile_carries_opaque_chunks_across_capability_roots() {
    let directory = tempfile::tempdir().unwrap();
    let left_root = directory.path().join("left-target");
    let right_root = directory.path().join("right-target");
    write_chunk_set(&left_root.join("repo/.engram"), &[("11111111", b"left")]);
    write_chunk_set(&right_root.join("repo/.engram"), &[("22222222", b"right")]);
    let left_fs = LocalTargetFilesystem::open(&left_root, RootAccess::ReadWrite).unwrap();
    let right_fs = LocalTargetFilesystem::open(&right_root, RootAccess::ReadWrite).unwrap();
    let target_declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    let receipt = EngramChunkAdapter::reconcile_targets(
        &left_fs,
        &target_declaration("macbook"),
        &right_fs,
        &target_declaration("vps"),
    )
    .unwrap();

    assert_eq!(receipt.moved.len(), 2);
    assert!(
        left_root
            .join("repo/.engram/chunks/22222222.jsonl.gz")
            .is_file()
    );
    assert!(
        right_root
            .join("repo/.engram/chunks/11111111.jsonl.gz")
            .is_file()
    );
}

struct MemorySsh {
    files: std::collections::BTreeMap<String, Vec<u8>>,
    syncs: Vec<EngramTargetSyncMode>,
    principal: Option<Principal>,
}

impl SshFilesystemTransport for MemorySsh {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        let files = &mut self.files;
        match request {
            SshFilesystemRequest::ResolvePrincipal { .. } => self
                .principal
                .clone()
                .map(|principal| SshFilesystemResponse::Principal { principal })
                .ok_or(TargetFilesystemError::PrincipalUnavailable),
            SshFilesystemRequest::ListDirectory { path, .. } => {
                let prefix = format!("{}/", path.as_str());
                let mut entries = files
                    .keys()
                    .filter_map(|candidate| candidate.strip_prefix(&prefix))
                    .filter(|candidate| !candidate.contains('/'))
                    .map(String::from)
                    .collect::<Vec<_>>();
                entries.sort();
                Ok(SshFilesystemResponse::Directory { entries })
            }
            SshFilesystemRequest::ReadFile { path, .. } => Ok(files
                .get(path.as_str())
                .cloned()
                .map(|content| SshFilesystemResponse::File { content })
                .unwrap_or(SshFilesystemResponse::Absent)),
            SshFilesystemRequest::WriteFile { path, content, .. } => {
                files.insert(path.to_string(), content);
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::Remove { path, .. } => {
                files.remove(path.as_str());
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::EngramSync { mode, .. } => {
                self.syncs.push(mode);
                Ok(SshFilesystemResponse::EngramSynced { mode })
            }
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }
}

#[test]
fn target_reconcile_uses_the_closed_ssh_filesystem_protocol() {
    let manifest = |id: &str| {
        serde_json::to_vec(&json!({"version":1,"chunks":[{
            "id":id,"created_by":"test","created_at":"2026-08-06T00:00:00Z",
            "sessions":0,"memories":1,"prompts":0
        }]}))
        .unwrap()
    };
    let remote = |id: &str, content: &[u8]| {
        SshTargetFilesystem::new(
            StableId::parse("repo-root").unwrap(),
            MemorySsh {
                files: [
                    ("repo/.engram/manifest.json".to_owned(), manifest(id)),
                    (
                        format!("repo/.engram/chunks/{id}.jsonl.gz"),
                        content.to_vec(),
                    ),
                ]
                .into_iter()
                .collect(),
                syncs: Vec::new(),
                principal: Some(Principal::parse("astemarie").unwrap()),
            },
        )
    };
    let left = remote("11111111", b"left");
    let right = remote("22222222", b"right");
    let declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    let receipt = EngramChunkAdapter::reconcile_targets(
        &left,
        &declaration("macbook"),
        &right,
        &declaration("vps"),
    )
    .unwrap();

    assert_eq!(receipt.moved.len(), 2);
    assert_eq!(
        EngramChunkAdapter::status_target(&right, &declaration("vps"))
            .unwrap()
            .present
            .len(),
        2
    );
}

#[test]
fn target_status_ignores_crash_left_stage_files_before_manifest_enumeration() {
    let manifest = serde_json::to_vec(&json!({"version":1,"chunks":[{
        "id":"11111111","created_by":"test","created_at":"2026-08-06T00:00:00Z",
        "sessions":0,"memories":1,"prompts":0
    }]}))
    .unwrap();
    let remote = SshTargetFilesystem::new(
        StableId::parse("repo-root").unwrap(),
        MemorySsh {
            files: [
                ("repo/.engram/manifest.json".to_owned(), manifest),
                (
                    "repo/.engram/.manifest.json.commonkit-tmp".to_owned(),
                    b"partial".to_vec(),
                ),
                (
                    "repo/.engram/chunks/11111111.jsonl.gz".to_owned(),
                    b"left".to_vec(),
                ),
                (
                    "repo/.engram/chunks/.22222222.jsonl.gz.commonkit-tmp".to_owned(),
                    b"partial".to_vec(),
                ),
            ]
            .into_iter()
            .collect(),
            syncs: Vec::new(),
            principal: Some(Principal::parse("astemarie").unwrap()),
        },
    );
    let declaration = EngramTargetChunkSetDeclaration {
        target_id: StableId::parse("macbook").unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    let status = EngramChunkAdapter::status_target(&remote, &declaration).unwrap();

    assert_eq!(status.state, EngramChunkSetState::InSync);
    assert_eq!(status.present.len(), 1);
}

#[test]
fn active_target_reconcile_drives_remote_export_and_import() {
    let manifest = |id: &str| {
        serde_json::to_vec(&json!({"version":1,"chunks":[{
            "id":id,"created_by":"test","created_at":"2026-08-06T00:00:00Z",
            "sessions":0,"memories":1,"prompts":0
        }]}))
        .unwrap()
    };
    let remote = |id: &str, content: &[u8]| {
        SshTargetFilesystem::new(
            StableId::parse("repo-root").unwrap(),
            MemorySsh {
                files: [
                    ("repo/.engram/manifest.json".to_owned(), manifest(id)),
                    (
                        format!("repo/.engram/chunks/{id}.jsonl.gz"),
                        content.to_vec(),
                    ),
                ]
                .into_iter()
                .collect(),
                syncs: Vec::new(),
                principal: Some(Principal::parse("astemarie").unwrap()),
            },
        )
    };
    let left = remote("11111111", b"left");
    let right = remote("22222222", b"right");
    let declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    let receipt = EngramChunkAdapter::reconcile_active_targets(
        &left,
        &declaration("macbook"),
        &right,
        &declaration("vps"),
    )
    .unwrap();

    assert_eq!(receipt.moved.len(), 2);
    assert_eq!(
        left.into_transport().unwrap().syncs,
        vec![EngramTargetSyncMode::Export, EngramTargetSyncMode::Import]
    );
    assert_eq!(
        right.into_transport().unwrap().syncs,
        vec![EngramTargetSyncMode::Export, EngramTargetSyncMode::Import]
    );
}

#[test]
fn unreachable_ssh_sync_is_an_explicit_unresolved_no_op() {
    struct Unreachable;
    impl SshFilesystemTransport for Unreachable {
        fn perform(
            &mut self,
            _: SshFilesystemRequest,
        ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
            Err(TargetFilesystemError::RemoteFailure {
                status: 255,
                message: "connection refused".to_owned(),
            })
        }
    }

    let left = SshTargetFilesystem::new(StableId::parse("left").unwrap(), Unreachable);
    let right = SshTargetFilesystem::new(StableId::parse("right").unwrap(), Unreachable);
    let declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    let receipt = EngramChunkAdapter::reconcile_active_targets(
        &left,
        &declaration("left"),
        &right,
        &declaration("right"),
    )
    .unwrap();

    assert_eq!(receipt.moved.len(), 0);
    assert_eq!(receipt.unresolved.as_deref(), Some("target is unreachable"));
}

#[test]
fn active_target_reconcile_does_not_hide_non_unreachable_failures() {
    #[derive(Clone, Copy)]
    enum Failure {
        ReadOnly,
        Malformed,
        Command,
    }
    struct Failing {
        failure: Failure,
    }
    impl SshFilesystemTransport for Failing {
        fn perform(
            &mut self,
            request: SshFilesystemRequest,
        ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
            match request {
                SshFilesystemRequest::ResolvePrincipal { .. } => {
                    Ok(SshFilesystemResponse::Principal {
                        principal: Principal::parse("astemarie").unwrap(),
                    })
                }
                SshFilesystemRequest::EngramSync { .. } => match self.failure {
                    Failure::ReadOnly => Err(TargetFilesystemError::ReadOnly),
                    Failure::Malformed => Ok(SshFilesystemResponse::Absent),
                    Failure::Command => Err(TargetFilesystemError::EngramCommandFailed),
                },
                _ => Err(TargetFilesystemError::InvalidRemoteResponse),
            }
        }
    }

    let declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };

    for (failure, expected) in [
        (Failure::ReadOnly, "target root is read-only"),
        (
            Failure::Malformed,
            "remote helper returned an invalid or unexpected response",
        ),
        (Failure::Command, "target Engram command failed"),
    ] {
        let left = SshTargetFilesystem::new(StableId::parse("left").unwrap(), Failing { failure });
        let right =
            SshTargetFilesystem::new(StableId::parse("right").unwrap(), Failing { failure });
        let error = EngramChunkAdapter::reconcile_active_targets(
            &left,
            &declaration("left"),
            &right,
            &declaration("right"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn active_target_reconcile_requires_an_attested_remote_owner_binding() {
    let declaration = |target: &str| EngramTargetChunkSetDeclaration {
        target_id: StableId::parse(target).unwrap(),
        project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
        owner_id: EngramOwnerId::try_from("github:astemarie").unwrap(),
        project_root: NormalizedManagedPath::parse("repo").unwrap(),
        root: NormalizedManagedPath::parse("repo/.engram").unwrap(),
        scope: EngramScope::Project,
    };
    for principal in [None, Some(Principal::parse("attacker").unwrap())] {
        let left = SshTargetFilesystem::new(
            StableId::parse("left").unwrap(),
            MemorySsh {
                files: Default::default(),
                syncs: Vec::new(),
                principal: principal.clone(),
            },
        );
        let right = SshTargetFilesystem::new(
            StableId::parse("right").unwrap(),
            MemorySsh {
                files: Default::default(),
                syncs: Vec::new(),
                principal: Some(Principal::parse("astemarie").unwrap()),
            },
        );
        let error = EngramChunkAdapter::reconcile_active_targets(
            &left,
            &declaration("left"),
            &right,
            &declaration("right"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            commonkit_adapters::EngramError::OwnerBindingMismatch
                | commonkit_adapters::EngramError::Target(
                    TargetFilesystemError::PrincipalUnavailable
                )
        ));
    }
}

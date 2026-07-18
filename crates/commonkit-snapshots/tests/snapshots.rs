use commonkit_snapshots::{
    Authority, DatabaseId, DeterministicTestCipher, InMemoryDatabaseTarget, InMemoryObjectStore,
    SnapshotCoordinator, SnapshotError, SnapshotService, StaticBackup,
};

#[test]
fn only_one_target_may_be_the_authoritative_writer() {
    let mut coordinator = SnapshotCoordinator::new(DatabaseId::new("context-mode").unwrap());

    coordinator.initialize_writer("workstation-a").unwrap();
    assert_eq!(
        coordinator.authority(),
        &Authority::Writer {
            target: "workstation-a".into()
        }
    );

    let error = coordinator.initialize_writer("workstation-b").unwrap_err();
    assert_eq!(
        error,
        SnapshotError::WriterAlreadyAssigned("workstation-a".into())
    );
}

#[test]
fn promotion_refuses_to_abandon_unsnapshotted_writer_changes() {
    let mut coordinator = SnapshotCoordinator::new(DatabaseId::new("context-mode").unwrap());
    coordinator.initialize_writer("workstation-a").unwrap();
    coordinator.record_snapshot_digest("sha256:snapshot");

    let error = coordinator
        .promote("workstation-b", "sha256:new-local-state", "sha256:snapshot")
        .unwrap_err();

    assert_eq!(error, SnapshotError::UnsnapshottedWriterChanges);
    assert_eq!(
        coordinator.authority(),
        &Authority::Writer {
            target: "workstation-a".into()
        }
    );
}

#[test]
fn promotion_requires_the_candidate_to_match_the_latest_snapshot() {
    let mut coordinator = SnapshotCoordinator::new(DatabaseId::new("context-mode").unwrap());
    coordinator.initialize_writer("workstation-a").unwrap();
    coordinator.record_snapshot_digest("sha256:snapshot");

    coordinator
        .promote("workstation-b", "sha256:snapshot", "sha256:snapshot")
        .unwrap();

    assert_eq!(
        coordinator.authority(),
        &Authority::Writer {
            target: "workstation-b".into()
        }
    );
}

#[test]
fn restore_stages_and_verifies_before_atomic_swap() {
    let database = DatabaseId::new("context-mode").unwrap();
    let cipher = DeterministicTestCipher::new([9; 32]);
    let service = SnapshotService::new(&cipher);
    let mut objects = InMemoryObjectStore::default();
    let manifest = service
        .snapshot(
            &database,
            "workstation-a",
            None,
            &StaticBackup::new("sqlite-backup-v1", b"new database".to_vec()),
            &mut objects,
        )
        .unwrap();
    let mut target = InMemoryDatabaseTarget::new(b"old database".to_vec());

    service.restore(&manifest, &objects, &mut target).unwrap();

    assert_eq!(target.current(), b"new database");
    assert_eq!(target.swap_count(), 1);
}

#[test]
fn failed_post_swap_verification_rolls_back_the_database() {
    let database = DatabaseId::new("context-mode").unwrap();
    let cipher = DeterministicTestCipher::new([11; 32]);
    let service = SnapshotService::new(&cipher);
    let mut objects = InMemoryObjectStore::default();
    let manifest = service
        .snapshot(
            &database,
            "workstation-a",
            None,
            &StaticBackup::new("sqlite-backup-v1", b"new database".to_vec()),
            &mut objects,
        )
        .unwrap();
    let mut target = InMemoryDatabaseTarget::new(b"old database".to_vec());
    target.fail_next_verification();

    assert_eq!(
        service
            .restore(&manifest, &objects, &mut target)
            .unwrap_err(),
        SnapshotError::RestoreVerificationFailed
    );
    assert_eq!(target.current(), b"old database");
}

#[test]
fn snapshot_encrypts_database_bytes_and_verifies_manifest_integrity() {
    let database = DatabaseId::new("context-mode").unwrap();
    let source = StaticBackup::new(
        "sqlite-backup-v1",
        b"SQLite format 3\0private rows".to_vec(),
    );
    let mut objects = InMemoryObjectStore::default();
    let cipher = DeterministicTestCipher::new([7; 32]);
    let service = SnapshotService::new(&cipher);

    let manifest = service
        .snapshot(&database, "workstation-a", None, &source, &mut objects)
        .unwrap();

    assert_eq!(manifest.schema, "commonkit.snapshot.v1");
    assert_eq!(manifest.source_format, "sqlite-backup-v1");
    assert_eq!(manifest.parent_digest, None);
    assert_eq!(manifest.source_digest, manifest.content_digest);
    assert!(!objects.contains_plaintext(b"private rows"));
    assert_eq!(
        service.verify_and_decrypt(&manifest, &objects).unwrap(),
        b"SQLite format 3\0private rows"
    );
}

#[test]
fn manifest_provenance_is_authenticated() {
    let database = DatabaseId::new("context-mode").unwrap();
    let cipher = DeterministicTestCipher::new([13; 32]);
    let service = SnapshotService::new(&cipher);
    let mut objects = InMemoryObjectStore::default();
    let mut manifest = service
        .snapshot(
            &database,
            "workstation-a",
            Some("sha256:parent".into()),
            &StaticBackup::new("sqlite-backup-v1", b"database".to_vec()),
            &mut objects,
        )
        .unwrap();

    manifest.parent_digest = Some("sha256:other-parent".into());

    assert_eq!(
        service.verify_and_decrypt(&manifest, &objects).unwrap_err(),
        SnapshotError::AuthenticationFailed
    );
}

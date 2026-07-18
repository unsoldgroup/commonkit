use commonkit_snapshots::{
    Authority, DatabaseId, DeterministicTestCipher, InMemoryDatabaseTarget, InMemoryObjectStore,
    SnapshotCoordinator, SnapshotError, SnapshotService, StaticBackup,
};

#[test]
fn production_cipher_uses_random_nonces_and_rejects_wrong_keys_or_tampering() {
    use commonkit_snapshots::{AuthenticatedCipher, XChaCha20Cipher};

    let cipher = XChaCha20Cipher::new([7; 32]);
    let first = cipher.seal(b"database", b"manifest").unwrap();
    let second = cipher.seal(b"database", b"manifest").unwrap();
    assert_ne!(first, second, "each snapshot requires a fresh nonce");
    assert_eq!(cipher.open(&first, b"manifest").unwrap(), b"database");

    let wrong_key = XChaCha20Cipher::new([8; 32]);
    assert!(wrong_key.open(&first, b"manifest").is_err());
    let mut tampered = first;
    *tampered.last_mut().unwrap() ^= 1;
    assert!(cipher.open(&tampered, b"manifest").is_err());
}

#[test]
fn sqlite_backup_is_consistent_and_integrity_checked_without_copying_wal_files() {
    use commonkit_snapshots::{ConsistentBackup, SqliteBackup};
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("context.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute("create table context(value text not null)", [])
        .unwrap();
    connection
        .execute("insert into context values ('portable')", [])
        .unwrap();

    let bytes = SqliteBackup::new(&database).export().unwrap();
    let restored = directory.path().join("restored.sqlite");
    std::fs::write(&restored, bytes).unwrap();
    let restored = Connection::open(restored).unwrap();
    assert_eq!(
        restored
            .query_row("select value from context", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "portable"
    );
    assert_eq!(
        restored
            .query_row("pragma integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn s3_compatible_store_uses_binary_stdio_and_fixed_argv_without_credentials() {
    use commonkit_snapshots::{ObjectCommandRunner, ObjectStore, S3CompatibleObjectStore};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Runner(Arc<Mutex<Vec<(Vec<String>, Vec<u8>)>>>);
    impl ObjectCommandRunner for Runner {
        fn run(&mut self, arguments: &[String], stdin: &[u8]) -> Result<Vec<u8>, SnapshotError> {
            self.0
                .lock()
                .unwrap()
                .push((arguments.to_vec(), stdin.to_vec()));
            Ok(
                if arguments
                    .get(2)
                    .is_some_and(|value| value.starts_with("s3://"))
                {
                    b"ciphertext".to_vec()
                } else {
                    Vec::new()
                },
            )
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut store = S3CompatibleObjectStore::new(
        Runner(calls.clone()),
        "https://account.r2.cloudflarestorage.com",
        "commonkit-snapshots",
        "v1",
    )
    .unwrap();
    let key = format!("sha256:{}", "a".repeat(64));
    store.put(&key, b"ciphertext").unwrap();
    assert_eq!(store.get(&key).unwrap(), b"ciphertext");
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].1, b"ciphertext");
    assert_eq!(calls[0].0[0..3], ["s3", "cp", "-"]);
    assert!(
        calls
            .iter()
            .all(|(arguments, _)| arguments.iter().all(|value| !value.contains("secret")))
    );
}

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

use std::fs;

use commonkit_snapshots::{
    manifest_digest, Authority, AuthorityStore, DatabaseId, DatabaseLifecycle,
    DeterministicTestCipher, DurableRestore, InMemoryObjectStore, PromotionFailpoint,
    PromotionPlan, RestoreFailpoint, RestorePlan, RestoreState, SnapshotError, SnapshotService,
    SqliteBackup, StaticBackup,
};

#[derive(Default)]
struct Lifecycle(Vec<&'static str>);

impl DatabaseLifecycle for Lifecycle {
    fn stop(&mut self) -> Result<(), SnapshotError> {
        self.0.push("stop");
        Ok(())
    }
    fn start(&mut self) -> Result<(), SnapshotError> {
        self.0.push("start");
        Ok(())
    }
}

fn fixture() -> (
    tempfile::TempDir,
    DeterministicTestCipher,
    InMemoryObjectStore,
    commonkit_snapshots::SnapshotManifest,
    RestorePlan,
) {
    let root = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([29; 32]);
    let mut objects = InMemoryObjectStore::default();
    let database = DatabaseId::new("context-mode").unwrap();
    let manifest = SnapshotService::new(&cipher)
        .snapshot(
            &database,
            "workstation-a",
            None,
            &StaticBackup::new("file", b"restored database".to_vec()),
            &mut objects,
        )
        .unwrap();
    let plan = RestorePlan {
        schema: "commonkit.restore-plan.v1".into(),
        run_id: "restore-run-1".into(),
        snapshot_id: "snapshot-1".into(),
        manifest_digest: manifest_digest(&manifest).unwrap(),
        database,
        expected_content_digest: manifest.content_digest.clone(),
    };
    (root, cipher, objects, manifest, plan)
}

#[test]
fn fresh_process_recovers_an_interrupted_swap_from_encrypted_preimage() {
    let (root, cipher, objects, manifest, plan) = fixture();
    let database = root.path().join("context.db");
    fs::write(&database, b"original database").unwrap();
    let mut lifecycle = Lifecycle::default();
    let error = DurableRestore::open(root.path().join("state"), &cipher)
        .unwrap()
        .execute(
            plan,
            &manifest,
            &objects,
            &database,
            &mut lifecycle,
            RestoreFailpoint::AfterSwap,
        )
        .unwrap_err();
    assert_eq!(error, SnapshotError::Interrupted);
    assert_eq!(fs::read(&database).unwrap(), b"restored database");

    let mut fresh_lifecycle = Lifecycle::default();
    let receipt = DurableRestore::open(root.path().join("state"), &cipher)
        .unwrap()
        .recover("restore-run-1", &database, &mut fresh_lifecycle)
        .unwrap();
    assert_eq!(receipt.state, RestoreState::RolledBack);
    assert_eq!(fs::read(database).unwrap(), b"original database");
    assert_eq!(fresh_lifecycle.0, ["stop", "start"]);
    let metadata =
        fs::read_to_string(root.path().join("state/runs/restore-run-1/receipt.json")).unwrap();
    assert!(!metadata.contains("original database"));
    assert!(!metadata.contains("restored database"));
}

#[test]
fn tampered_receipt_and_preimage_fail_closed() {
    let (root, cipher, objects, manifest, plan) = fixture();
    let database = root.path().join("context.db");
    fs::write(&database, b"original database").unwrap();
    DurableRestore::open(root.path().join("state"), &cipher)
        .unwrap()
        .execute(
            plan,
            &manifest,
            &objects,
            &database,
            &mut Lifecycle::default(),
            RestoreFailpoint::AfterSwap,
        )
        .unwrap_err();
    let receipt_path = root.path().join("state/runs/restore-run-1/receipt.json");
    let mut receipt = fs::read(&receipt_path).unwrap();
    let middle = receipt.len() / 2;
    receipt[middle] ^= 1;
    fs::write(&receipt_path, receipt).unwrap();
    assert_eq!(
        DurableRestore::open(root.path().join("state"), &cipher)
            .unwrap()
            .recover("restore-run-1", &database, &mut Lifecycle::default())
            .unwrap_err(),
        SnapshotError::TransactionIntegrity
    );
}

#[test]
fn sqlite_wal_snapshot_restores_as_an_integrity_checked_database() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.sqlite");
    let source_connection = rusqlite::Connection::open(&source).unwrap();
    source_connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    source_connection
        .execute("create table context(value text not null)", [])
        .unwrap();
    source_connection
        .execute("insert into context values ('from-wal')", [])
        .unwrap();
    let database_id = DatabaseId::new("context-mode").unwrap();
    let cipher = DeterministicTestCipher::new([31; 32]);
    let mut objects = InMemoryObjectStore::default();
    let manifest = SnapshotService::new(&cipher)
        .snapshot(
            &database_id,
            "writer-a",
            None,
            &SqliteBackup::new(&source),
            &mut objects,
        )
        .unwrap();
    let destination = root.path().join("destination.sqlite");
    let old = rusqlite::Connection::open(&destination).unwrap();
    old.execute("create table old(value text)", []).unwrap();
    drop(old);
    let plan = RestorePlan {
        schema: "commonkit.restore-plan.v1".into(),
        run_id: "sqlite-restore".into(),
        snapshot_id: "snapshot-sqlite".into(),
        manifest_digest: manifest_digest(&manifest).unwrap(),
        database: database_id,
        expected_content_digest: manifest.content_digest.clone(),
    };
    DurableRestore::open(root.path().join("state"), &cipher)
        .unwrap()
        .execute(
            plan,
            &manifest,
            &objects,
            &destination,
            &mut Lifecycle::default(),
            RestoreFailpoint::None,
        )
        .unwrap();
    let restored = rusqlite::Connection::open(destination).unwrap();
    assert_eq!(
        restored
            .query_row("select value from context", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "from-wal"
    );
    assert_eq!(
        restored
            .query_row("pragma integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn writer_promotion_is_plan_bound_receipted_and_tamper_evident() {
    let root = tempfile::tempdir().unwrap();
    let database = DatabaseId::new("context-mode").unwrap();
    let store = AuthorityStore::open(root.path()).unwrap();
    store.initialize(&database, "writer-a").unwrap();
    let receipt = store
        .promote(PromotionPlan {
            schema: "commonkit.promotion-plan.v1".into(),
            run_id: "promotion-1".into(),
            database: database.clone(),
            previous_writer: "writer-a".into(),
            candidate_writer: "writer-b".into(),
            latest_snapshot_digest: "sha256:snapshot".into(),
            current_writer_digest: "sha256:snapshot".into(),
            candidate_digest: "sha256:snapshot".into(),
        })
        .unwrap();
    assert_eq!(receipt.authoritative_writer, "writer-b");
    assert_eq!(
        store.authority(&database).unwrap(),
        Authority::Writer {
            target: "writer-b".into()
        }
    );
    let authority = root.path().join("authority-context-mode.json");
    let mut bytes = fs::read(&authority).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    fs::write(authority, bytes).unwrap();
    assert_eq!(
        store.authority(&database).unwrap_err(),
        SnapshotError::TransactionIntegrity
    );
}

#[test]
fn fresh_process_finishes_a_promotion_interrupted_after_authority_write() {
    let root = tempfile::tempdir().unwrap();
    let database = DatabaseId::new("context-mode").unwrap();
    let store = AuthorityStore::open(root.path()).unwrap();
    store.initialize(&database, "writer-a").unwrap();
    let plan = PromotionPlan {
        schema: "commonkit.promotion-plan.v1".into(),
        run_id: "promotion-crash".into(),
        database,
        previous_writer: "writer-a".into(),
        candidate_writer: "writer-b".into(),
        latest_snapshot_digest: "sha256:snapshot".into(),
        current_writer_digest: "sha256:snapshot".into(),
        candidate_digest: "sha256:snapshot".into(),
    };
    assert_eq!(
        store
            .promote_with_failpoint(plan, PromotionFailpoint::AfterAuthorityWrite)
            .unwrap_err(),
        SnapshotError::Interrupted
    );
    let receipt = AuthorityStore::open(root.path())
        .unwrap()
        .recover_promotion("promotion-crash")
        .unwrap();
    assert_eq!(receipt.authoritative_writer, "writer-b");
    assert!(root
        .path()
        .join("promotions/promotion-crash/receipt.json")
        .is_file());
}

#[test]
fn tampered_manifest_or_encrypted_preimage_never_reaches_live_state() {
    let (root, cipher, objects, mut manifest, plan) = fixture();
    let database = root.path().join("context.db");
    fs::write(&database, b"original database").unwrap();
    manifest.source_target = "attacker".into();
    assert_eq!(
        DurableRestore::open(root.path().join("state"), &cipher)
            .unwrap()
            .execute(
                plan.clone(),
                &manifest,
                &objects,
                &database,
                &mut Lifecycle::default(),
                RestoreFailpoint::None,
            )
            .unwrap_err(),
        SnapshotError::TransactionIntegrity
    );
    assert_eq!(fs::read(&database).unwrap(), b"original database");

    let (_, cipher, objects, manifest, plan) = fixture();
    DurableRestore::open(root.path().join("state-2"), &cipher)
        .unwrap()
        .execute(
            plan,
            &manifest,
            &objects,
            &database,
            &mut Lifecycle::default(),
            RestoreFailpoint::AfterSwap,
        )
        .unwrap_err();
    let object = fs::read_dir(root.path().join("state-2/objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = fs::read(&object).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    fs::write(object, bytes).unwrap();
    assert_eq!(
        DurableRestore::open(root.path().join("state-2"), &cipher)
            .unwrap()
            .recover("restore-run-1", &database, &mut Lifecycle::default())
            .unwrap_err(),
        SnapshotError::IntegrityMismatch
    );
}

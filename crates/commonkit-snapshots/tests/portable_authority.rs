use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use commonkit_snapshots::{
    DatabaseId, DeterministicTestCipher, PortableAuthorityStore, SnapshotError,
};
use sha2::{Digest, Sha256};

fn descriptor(root: &std::path::Path, bytes: &[u8]) -> String {
    fs::create_dir_all(root.join("snapshots")).unwrap();
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    fs::write(
        root.join("snapshots")
            .join(format!("{}.json", &digest[7..])),
        bytes,
    )
    .unwrap();
    digest
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[test]
fn promotion_on_machine_b_revokes_machine_a_and_fresh_clone_reads_head() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([71; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store_a = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store_a.initialize(&database, "machine-a").unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    let head = digest(b"one");
    let first = store_a
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &head,
            &first_descriptor,
        )
        .unwrap();

    let store_b = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let promoted = store_b
        .compare_and_swap_writer(&database, &first.revision, "machine-a", "machine-b")
        .unwrap();
    assert_eq!(promoted.record.current_writer, "machine-b");
    assert_eq!(
        promoted.record.accepted_head.as_deref(),
        Some(head.as_str())
    );

    assert_eq!(
        store_a.compare_and_swap_head(
            &database,
            &first.revision,
            "machine-a",
            &digest(b"fork"),
            &first_descriptor,
        ),
        Err(SnapshotError::StalePortableAuthority)
    );

    let clone = tempfile::tempdir().unwrap();
    copy_tree(portable.path(), clone.path());
    let fresh = PortableAuthorityStore::open(clone.path(), &cipher)
        .unwrap()
        .read(&database)
        .unwrap();
    assert_eq!(fresh.record.current_writer, "machine-b");
    assert_eq!(fresh.record.accepted_head.as_deref(), Some(head.as_str()));
    assert_eq!(fresh.revision, promoted.revision);
}

#[test]
fn concurrent_promotions_have_one_cas_winner() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([72; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let current = store.initialize(&database, "machine-a").unwrap();
    let portable = Arc::new(portable);
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for candidate in ["machine-b", "machine-c"] {
        let portable = Arc::clone(&portable);
        let barrier = Arc::clone(&barrier);
        let database = database.clone();
        let revision = current.revision.clone();
        handles.push(thread::spawn(move || {
            let cipher = DeterministicTestCipher::new([72; 32]);
            let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
            barrier.wait();
            store.compare_and_swap_writer(&database, &revision, "machine-a", candidate)
        }));
    }
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(SnapshotError::StalePortableAuthority))
            .count(),
        1
    );
}

#[test]
fn deleting_or_rolling_back_the_accepted_descriptor_fails_closed() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([73; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    store
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &digest(b"one"),
            &first_descriptor,
        )
        .unwrap();

    fs::remove_file(
        portable
            .path()
            .join("snapshots")
            .join(format!("{}.json", &first_descriptor[7..])),
    )
    .unwrap();
    assert_eq!(
        store.read(&database),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

#[test]
fn recomputing_an_unkeyed_digest_cannot_forge_portable_writer_authority() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([74; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    store.initialize(&database, "machine-a").unwrap();

    let path = portable.path().join("authority/context-mode.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["value"]["currentWriter"] = serde_json::json!("attacker");
    envelope["digest"] = serde_json::json!(digest(&serde_jcs::to_vec(&envelope["value"]).unwrap()));
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

    assert_eq!(
        store.read(&database),
        Err(SnapshotError::TransactionIntegrity)
    );
}

#[test]
fn replaying_an_older_authenticated_authority_is_rejected_when_history_remains() {
    let portable = tempfile::tempdir().unwrap();
    let cipher = DeterministicTestCipher::new([75; 32]);
    let database = DatabaseId::new("context-mode").unwrap();
    let store = PortableAuthorityStore::open(portable.path(), &cipher).unwrap();
    let initial = store.initialize(&database, "machine-a").unwrap();
    let old_authority = fs::read(portable.path().join("authority/context-mode.json")).unwrap();
    let first_descriptor = descriptor(portable.path(), br#"{"snapshot":"one"}"#);
    store
        .compare_and_swap_head(
            &database,
            &initial.revision,
            "machine-a",
            &digest(b"one"),
            &first_descriptor,
        )
        .unwrap();

    fs::write(
        portable.path().join("authority/context-mode.json"),
        old_authority,
    )
    .unwrap();

    assert_eq!(
        store.read(&database),
        Err(SnapshotError::PortableAuthorityRollback)
    );
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

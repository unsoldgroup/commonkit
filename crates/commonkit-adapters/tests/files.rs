use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{FileAdapter, FileIntent, ManagedRelativePath};
use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_reconcile::Adapter;
use sha2::{Digest, Sha256};

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}")).expect("digest")
}

fn intent(path: &str, content: &[u8], expected_before: Option<Sha256Digest>) -> FileIntent {
    FileIntent {
        id: StableId::parse("managed-config").expect("id"),
        path: ManagedRelativePath::parse(path).expect("path"),
        content: content.to_vec(),
        expected_before,
    }
}

#[test]
fn rejects_absolute_traversal_windows_and_git_paths() {
    for path in [
        "",
        "/etc/passwd",
        "../outside",
        "a/../outside",
        "C:\\secret",
        ".git/config",
    ] {
        assert!(ManagedRelativePath::parse(path).is_err(), "accepted {path}");
    }
    assert!(ManagedRelativePath::parse("agents/config.json").is_ok());
}

#[test]
fn creates_verifies_and_removes_a_managed_file_on_rollback() {
    let root = temporary_directory("file-create");
    let target = root.join("target");
    let state = root.join("state");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register(intent("nested/config.json", b"{\"enabled\":true}\n", None))
        .expect("operation");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert_eq!(
        fs::read(target.join("nested/config.json")).expect("file"),
        b"{\"enabled\":true}\n"
    );
    adapter.rollback(&operation).expect("rollback");
    assert!(!target.join("nested/config.json").exists());

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn restores_the_exact_preimage_and_rejects_changes_after_planning() {
    let root = temporary_directory("file-update");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(&target).expect("target");
    fs::write(target.join("config.txt"), b"before").expect("preimage");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register(intent("config.txt", b"after", Some(digest(b"before"))))
        .expect("operation");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    adapter.rollback(&operation).expect("rollback");
    assert_eq!(
        fs::read(target.join("config.txt")).expect("file"),
        b"before"
    );

    fs::write(target.join("config.txt"), b"changed externally").expect("drift");
    assert_eq!(
        adapter.prepare(&operation).expect_err("must reject").code,
        "preimage_changed"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn rejects_literal_secrets_in_portable_file_content() {
    let root = temporary_directory("file-secret");
    let mut adapter =
        FileAdapter::open(&root.join("target"), &root.join("state")).expect("adapter");
    assert!(
        adapter
            .register(intent("config.txt", b"API_KEY=literal-secret", None))
            .is_err()
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{
    FileAdapter, FileMode, FilesystemIntent, NormalizedManagedPath, SafeSymlinkTarget,
};
use commonkit_contracts::StableId;
use commonkit_reconcile::Adapter;

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

fn id(value: &str) -> StableId {
    StableId::parse(value).expect("id")
}

#[test]
fn applies_and_rolls_back_directory_symlink_and_removal_resources() {
    let root = temporary_directory("semantic-filesystem-resources");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(target.join("old-dir")).expect("target");
    fs::write(target.join("obsolete"), b"restore me").expect("obsolete preimage");
    symlink("old-dir", target.join("current")).expect("symlink preimage");

    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let directory = adapter
        .register_resource(
            id("managed-directory"),
            FilesystemIntent::Directory {
                path: NormalizedManagedPath::parse("config").expect("path"),
                mode: Some(FileMode::parse(0o700).expect("mode")),
                exact: false,
            },
        )
        .expect("directory operation");
    let symlink_operation = adapter
        .register_resource(
            id("managed-symlink"),
            FilesystemIntent::Symlink {
                path: NormalizedManagedPath::parse("current").expect("path"),
                target: SafeSymlinkTarget::parse(
                    &NormalizedManagedPath::parse("current").expect("path"),
                    "config",
                )
                .expect("target"),
                expected_before: None,
            },
        )
        .expect("symlink operation");
    let removal = adapter
        .register_resource(
            id("managed-removal"),
            FilesystemIntent::Remove {
                path: NormalizedManagedPath::parse("obsolete").expect("path"),
                expected_before: None,
            },
        )
        .expect("remove operation");

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh apply adapter");

    for operation in [&directory, &symlink_operation, &removal] {
        adapter.prepare(operation).expect("prepare");
        adapter.apply(operation).expect("apply");
        adapter.verify(operation).expect("verify");
    }
    assert!(target.join("config").is_dir());
    assert_eq!(
        fs::metadata(target.join("config"))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::read_link(target.join("current")).expect("link"),
        PathBuf::from("config")
    );
    assert!(!target.join("obsolete").exists());

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh rollback adapter");
    for operation in [&removal, &symlink_operation, &directory] {
        adapter.rollback(operation).expect("rollback");
    }
    assert!(!target.join("config").exists());
    assert_eq!(
        fs::read_link(target.join("current")).expect("restored link"),
        PathBuf::from("old-dir")
    );
    assert_eq!(
        fs::read(target.join("obsolete")).expect("restored file"),
        b"restore me"
    );

    fs::remove_dir_all(root).expect("cleanup");
}

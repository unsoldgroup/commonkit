#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{
    ArtifactStore, ContentSensitivity, FileAdapter, FileAdapterError, FileMode, FilesystemIntent,
    NormalizedManagedPath, SafeSymlinkTarget,
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

fn file_operation(
    adapter: &mut FileAdapter,
    root: &std::path::Path,
    path: &str,
    bytes: &[u8],
) -> commonkit_contracts::Operation {
    let provider = ArtifactStore::open(root.join("provider-artifacts")).expect("provider store");
    let content = provider
        .put(bytes, ContentSensitivity::Portable)
        .expect("provider content");
    adapter
        .register_materialized_resource(
            id("managed-file"),
            FilesystemIntent::File {
                path: NormalizedManagedPath::parse(path).expect("path"),
                content,
                mode: Some(FileMode::parse(0o600).expect("mode")),
                expected_before: None,
            },
            &provider,
        )
        .expect("file operation")
}

#[test]
fn unspecified_file_mode_does_not_create_perpetual_drift() {
    let root = temporary_directory("unspecified-file-mode-no-drift");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(target.join("home")).expect("target");
    fs::write(target.join("home/config.txt"), b"managed\n").expect("managed file");

    let provider = ArtifactStore::open(root.join("provider-artifacts")).expect("provider store");
    let content = provider
        .put(b"managed\n", ContentSensitivity::Portable)
        .expect("provider content");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");

    assert!(matches!(
        adapter.register_materialized_resource(
            id("managed-file"),
            FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/config.txt").expect("path"),
                content,
                mode: None,
                expected_before: None,
            },
            &provider,
        ),
        Err(FileAdapterError::NoChange)
    ));

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn unspecified_file_mode_updates_and_rolls_back_without_changing_the_original_mode() {
    let root = temporary_directory("unspecified-file-mode-lifecycle");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(target.join("home")).expect("target");
    let managed_path = target.join("home/config.txt");
    fs::write(&managed_path, b"original\n").expect("original file");
    fs::set_permissions(&managed_path, fs::Permissions::from_mode(0o640)).expect("original mode");

    let provider = ArtifactStore::open(root.join("provider-artifacts")).expect("provider store");
    let content = provider
        .put(b"managed\n", ContentSensitivity::Portable)
        .expect("provider content");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register_materialized_resource(
            id("managed-file"),
            FilesystemIntent::File {
                path: NormalizedManagedPath::parse("home/config.txt").expect("path"),
                content,
                mode: None,
                expected_before: None,
            },
            &provider,
        )
        .expect("file update");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert_eq!(
        fs::read(&managed_path).expect("managed bytes"),
        b"managed\n"
    );

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh rollback adapter");
    adapter.rollback(&operation).expect("rollback");
    assert_eq!(
        fs::read(&managed_path).expect("restored bytes"),
        b"original\n"
    );
    assert_eq!(
        fs::metadata(&managed_path)
            .expect("restored metadata")
            .permissions()
            .mode()
            & 0o777,
        0o640
    );

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn unspecified_directory_mode_updates_and_rolls_back_an_existing_file_with_its_mode() {
    let root = temporary_directory("unspecified-directory-mode-lifecycle");
    let target = root.join("target");
    let state = root.join("state");
    fs::create_dir_all(&target).expect("target");
    let managed_path = target.join("config");
    fs::write(&managed_path, b"original file\n").expect("original file");
    fs::set_permissions(&managed_path, fs::Permissions::from_mode(0o640)).expect("original mode");

    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register_resource(
            id("managed-directory"),
            FilesystemIntent::Directory {
                path: NormalizedManagedPath::parse("config").expect("path"),
                mode: None,
                exact: false,
            },
        )
        .expect("directory update");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert!(managed_path.is_dir());

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh rollback adapter");
    adapter.rollback(&operation).expect("rollback");
    assert_eq!(
        fs::read(&managed_path).expect("restored bytes"),
        b"original file\n"
    );
    assert_eq!(
        fs::metadata(&managed_path)
            .expect("restored metadata")
            .permissions()
            .mode()
            & 0o777,
        0o640
    );

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn unspecified_file_mode_updates_and_rolls_back_an_existing_directory_with_its_mode() {
    let root = temporary_directory("unspecified-mode-directory-preimage");
    let target = root.join("target");
    let state = root.join("state");
    let managed_path = target.join("config");
    fs::create_dir_all(&managed_path).expect("original directory");
    fs::set_permissions(&managed_path, fs::Permissions::from_mode(0o750))
        .expect("original directory mode");

    let provider = ArtifactStore::open(root.join("provider-artifacts")).expect("provider store");
    let content = provider
        .put(b"managed file\n", ContentSensitivity::Portable)
        .expect("provider content");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register_materialized_resource(
            id("managed-file"),
            FilesystemIntent::File {
                path: NormalizedManagedPath::parse("config").expect("path"),
                content,
                mode: None,
                expected_before: None,
            },
            &provider,
        )
        .expect("file update");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    adapter.verify(&operation).expect("verify");
    assert_eq!(
        fs::read(&managed_path).expect("managed bytes"),
        b"managed file\n"
    );

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh rollback adapter");
    adapter.rollback(&operation).expect("rollback");
    assert!(managed_path.is_dir());
    assert_eq!(
        fs::metadata(&managed_path)
            .expect("restored metadata")
            .permissions()
            .mode()
            & 0o777,
        0o750
    );

    fs::remove_dir_all(root).expect("cleanup");
}

// A project memory store is a managed directory whose contents CommonKit never
// created. Rollback restores the directory's declaration, never its contents,
// so a populated directory survives instead of failing ENOTEMPTY.
#[test]
fn rollback_leaves_a_populated_managed_directory_and_its_contents_in_place() {
    let root = temporary_directory("rollback-populated-directory");
    let target = root.join("target");
    let state = root.join("state");
    let managed_path = target.join("store");
    fs::create_dir_all(&target).expect("target root");

    let provider = ArtifactStore::open(root.join("provider-artifacts")).expect("provider store");
    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = adapter
        .register_materialized_resource(
            id("memory-store"),
            FilesystemIntent::Directory {
                path: NormalizedManagedPath::parse("store").expect("path"),
                mode: Some(FileMode::parse(0o700).expect("mode")),
                exact: false,
            },
            &provider,
        )
        .expect("directory creation");

    adapter.prepare(&operation).expect("prepare");
    adapter.apply(&operation).expect("apply");
    assert!(managed_path.is_dir());

    // The writer fills the store after the plan applied.
    let captured = managed_path.join("sessions.db");
    fs::write(&captured, b"captured memory\n").expect("writer output");

    drop(adapter);
    let mut adapter = FileAdapter::open(&target, &state).expect("fresh rollback adapter");
    adapter
        .rollback(&operation)
        .expect("rollback declines rather than failing on a populated directory");

    assert!(managed_path.is_dir(), "store directory survives rollback");
    assert_eq!(
        fs::read(&captured).expect("captured bytes survive rollback"),
        b"captured memory\n"
    );

    fs::remove_dir_all(root).expect("cleanup");
}

fn assert_adapter_did_not_read(state: &std::path::Path, forbidden: &[u8]) {
    let artifacts = state.join("artifacts");
    if !artifacts.exists() {
        return;
    }
    for entry in fs::read_dir(artifacts).expect("adapter artifacts") {
        let path = entry.expect("artifact entry").path();
        if path.is_file() {
            assert_ne!(
                fs::read(path).expect("artifact bytes"),
                forbidden,
                "unsafe target bytes entered the adapter artifact store"
            );
        }
    }
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

#[test]
fn semantic_apply_rejects_an_ancestor_substitution_without_mutating_outside() {
    let root = temporary_directory("semantic-apply-ancestor-substitution");
    let target = root.join("target");
    let state = root.join("state");
    let outside = root.join("outside");
    let displaced = root.join("displaced");
    fs::create_dir_all(target.join("config")).expect("target ancestor");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(outside.join("settings"), b"outside sentinel").expect("sentinel");

    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = file_operation(&mut adapter, &root, "config/settings", b"managed");
    adapter.prepare(&operation).expect("prepare");
    fs::rename(target.join("config"), &displaced).expect("displace ancestor");
    symlink(&outside, target.join("config")).expect("substitute ancestor");

    let error = adapter
        .apply(&operation)
        .expect_err("substituted ancestor must fail closed");
    assert!(matches!(
        error.code.as_str(),
        "unsafe_path" | "preimage_changed"
    ));
    assert_eq!(
        fs::read(outside.join("settings")).expect("outside sentinel"),
        b"outside sentinel"
    );
    assert!(!displaced.join("settings").exists());
    assert_adapter_did_not_read(&state, b"outside sentinel");

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn semantic_apply_rejects_a_target_root_substitution_without_mutating_outside() {
    let root = temporary_directory("semantic-apply-root-substitution");
    let target = root.join("target");
    let state = root.join("state");
    let outside = root.join("outside");
    let displaced = root.join("displaced-target");
    fs::create_dir_all(&target).expect("target");
    fs::create_dir_all(&outside).expect("outside");

    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = file_operation(&mut adapter, &root, "settings", b"managed");
    adapter.prepare(&operation).expect("prepare");
    fs::rename(&target, &displaced).expect("displace target root");
    symlink(&outside, &target).expect("substitute target root");

    let error = adapter
        .apply(&operation)
        .expect_err("substituted root must fail closed");
    assert!(matches!(
        error.code.as_str(),
        "unsafe_path" | "preimage_changed"
    ));
    assert!(!outside.join("settings").exists());
    assert!(!displaced.join("settings").exists());
    assert_adapter_did_not_read(&state, b"outside sentinel");

    fs::remove_file(&target).expect("cleanup substitute");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn semantic_apply_rejects_a_leaf_substitution_without_mutating_outside() {
    let root = temporary_directory("semantic-apply-leaf-substitution");
    let target = root.join("target");
    let state = root.join("state");
    let outside = root.join("outside");
    fs::create_dir_all(&target).expect("target");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(target.join("settings"), b"original").expect("preimage");
    fs::write(outside.join("sentinel"), b"outside sentinel").expect("sentinel");

    let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
    let operation = file_operation(&mut adapter, &root, "settings", b"managed");
    adapter.prepare(&operation).expect("prepare");
    fs::remove_file(target.join("settings")).expect("remove leaf");
    symlink(outside.join("sentinel"), target.join("settings")).expect("substitute leaf");

    let error = adapter
        .apply(&operation)
        .expect_err("substituted leaf must fail closed");
    assert!(matches!(
        error.code.as_str(),
        "unsafe_path" | "preimage_changed"
    ));
    assert_eq!(
        fs::read(outside.join("sentinel")).expect("outside sentinel"),
        b"outside sentinel"
    );
    assert!(
        fs::symlink_metadata(target.join("settings"))
            .expect("substituted leaf")
            .file_type()
            .is_symlink()
    );
    assert_adapter_did_not_read(&state, b"outside sentinel");

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn semantic_rollback_rejects_root_ancestor_and_leaf_substitution_without_outside_mutation() {
    for substitution in ["root", "ancestor", "leaf"] {
        let root = temporary_directory(&format!("semantic-rollback-{substitution}"));
        let target = root.join("target");
        let state = root.join("state");
        let outside = root.join("outside");
        let displaced = root.join("displaced");
        fs::create_dir_all(target.join("config")).expect("target");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("settings"), b"outside sentinel").expect("sentinel");
        let managed_path = if substitution == "root" {
            "settings"
        } else {
            "config/settings"
        };

        let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
        let operation = file_operation(&mut adapter, &root, managed_path, b"managed");
        adapter.prepare(&operation).expect("prepare");
        adapter.apply(&operation).expect("apply");

        match substitution {
            "root" => {
                fs::rename(&target, &displaced).expect("displace target");
                symlink(&outside, &target).expect("substitute target");
            }
            "ancestor" => {
                fs::rename(target.join("config"), &displaced).expect("displace ancestor");
                symlink(&outside, target.join("config")).expect("substitute ancestor");
            }
            "leaf" => {
                fs::remove_file(target.join("config/settings")).expect("remove leaf");
                symlink(outside.join("settings"), target.join("config/settings"))
                    .expect("substitute leaf");
            }
            _ => unreachable!(),
        }

        let error = adapter
            .rollback(&operation)
            .expect_err("substituted rollback path must fail closed");
        assert!(matches!(
            error.code.as_str(),
            "unsafe_path" | "rollback_preimage_changed"
        ));
        assert_eq!(
            fs::read(outside.join("settings")).expect("outside sentinel"),
            b"outside sentinel"
        );
        assert_adapter_did_not_read(&state, b"outside sentinel");

        if target
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            fs::remove_file(&target).expect("cleanup target link");
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}

#[test]
fn semantic_prepare_rejects_root_ancestor_and_leaf_substitution_without_reading_outside() {
    for substitution in ["root", "ancestor", "leaf"] {
        let root = temporary_directory(&format!("semantic-prepare-{substitution}"));
        let target = root.join("target");
        let state = root.join("state");
        let outside = root.join("outside");
        let displaced = root.join("displaced");
        fs::create_dir_all(target.join("config")).expect("target");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("settings"), b"outside prepare sentinel").expect("sentinel");
        let managed_path = if substitution == "root" {
            "settings"
        } else {
            "config/settings"
        };
        let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
        let operation = file_operation(&mut adapter, &root, managed_path, b"managed");

        match substitution {
            "root" => {
                fs::rename(&target, &displaced).expect("displace target");
                symlink(&outside, &target).expect("substitute target");
            }
            "ancestor" => {
                fs::rename(target.join("config"), &displaced).expect("displace ancestor");
                symlink(&outside, target.join("config")).expect("substitute ancestor");
            }
            "leaf" => {
                symlink(outside.join("settings"), target.join("config/settings"))
                    .expect("substitute leaf");
            }
            _ => unreachable!(),
        }

        let error = adapter
            .prepare(&operation)
            .expect_err("substituted prepare path must fail closed");
        assert_eq!(error.code.as_str(), "unsafe_path");
        assert_adapter_did_not_read(&state, b"outside prepare sentinel");

        if target
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::remove_file(&target).expect("cleanup target link");
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}

#[test]
fn semantic_verify_rejects_root_ancestor_and_leaf_substitution_without_reading_outside() {
    for substitution in ["root", "ancestor", "leaf"] {
        let root = temporary_directory(&format!("semantic-verify-{substitution}"));
        let target = root.join("target");
        let state = root.join("state");
        let outside = root.join("outside");
        let displaced = root.join("displaced");
        fs::create_dir_all(target.join("config")).expect("target");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("settings"), b"outside verify sentinel").expect("sentinel");
        let managed_path = if substitution == "root" {
            "settings"
        } else {
            "config/settings"
        };
        let mut adapter = FileAdapter::open(&target, &state).expect("adapter");
        let operation = file_operation(&mut adapter, &root, managed_path, b"managed");
        adapter.prepare(&operation).expect("prepare");
        adapter.apply(&operation).expect("apply");

        match substitution {
            "root" => {
                fs::rename(&target, &displaced).expect("displace target");
                symlink(&outside, &target).expect("substitute target");
            }
            "ancestor" => {
                fs::rename(target.join("config"), &displaced).expect("displace ancestor");
                symlink(&outside, target.join("config")).expect("substitute ancestor");
            }
            "leaf" => {
                fs::remove_file(target.join("config/settings")).expect("remove leaf");
                symlink(outside.join("settings"), target.join("config/settings"))
                    .expect("substitute leaf");
            }
            _ => unreachable!(),
        }

        let error = adapter
            .verify(&operation)
            .expect_err("substituted verify path must fail closed");
        assert_eq!(error.code.as_str(), "unsafe_path");
        assert_adapter_did_not_read(&state, b"outside verify sentinel");

        if target
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::remove_file(&target).expect("cleanup target link");
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}

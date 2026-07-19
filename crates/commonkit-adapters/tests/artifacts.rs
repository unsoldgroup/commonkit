use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_adapters::{ArtifactError, ArtifactStore, ContentSensitivity};

fn temporary_directory(test: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-{test}-{}-{nonce}", std::process::id()))
}

#[test]
fn content_addressed_artifacts_are_immutable_and_verified_on_every_load() {
    let root = temporary_directory("artifacts");
    let store = ArtifactStore::open(&root).expect("store");
    let reference = store
        .put(b"provider output", ContentSensitivity::Portable)
        .expect("put");
    assert_eq!(store.load(&reference).expect("load"), b"provider output");
    assert_eq!(
        store
            .put(b"provider output", ContentSensitivity::Portable)
            .expect("idempotent"),
        reference
    );

    let path = fs::read_dir(&root)
        .expect("artifacts")
        .next()
        .expect("artifact")
        .expect("entry")
        .path();
    fs::write(path, b"substituted bytes").expect("tamper");
    assert!(matches!(
        store.load(&reference),
        Err(ArtifactError::DigestMismatch)
    ));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn opening_an_existing_store_never_creates_a_missing_root() {
    let root = temporary_directory("missing-existing-artifacts");

    assert!(matches!(
        ArtifactStore::open_existing(&root),
        Err(ArtifactError::InvalidRoot)
    ));
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn opening_an_existing_store_rejects_unsafe_metadata_without_repairing_it() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temporary_directory("unsafe-existing-artifacts");
    fs::create_dir_all(&root).expect("root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("unsafe mode");
    let before = fs::symlink_metadata(&root)
        .expect("before")
        .permissions()
        .mode()
        & 0o777;

    assert!(matches!(
        ArtifactStore::open_existing(&root),
        Err(ArtifactError::InvalidRoot)
    ));
    assert_eq!(
        fs::symlink_metadata(&root)
            .expect("after")
            .permissions()
            .mode()
            & 0o777,
        before
    );

    let link = temporary_directory("existing-artifact-link");
    symlink(&root, &link).expect("link");
    assert!(matches!(
        ArtifactStore::open_existing(&link),
        Err(ArtifactError::InvalidRoot)
    ));
    assert!(
        fs::symlink_metadata(&link)
            .expect("link remains")
            .file_type()
            .is_symlink()
    );

    fs::remove_file(link).expect("cleanup link");
    fs::remove_dir_all(root).expect("cleanup root");
}

#[cfg(unix)]
#[test]
fn creating_a_store_rejects_a_symlink_root_instead_of_reopening_its_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let outside = temporary_directory("artifact-create-outside");
    let link = temporary_directory("artifact-create-link");
    fs::create_dir_all(&outside).expect("outside");
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).expect("private outside");
    symlink(&outside, &link).expect("substitute root");

    assert!(matches!(
        ArtifactStore::open(&link),
        Err(ArtifactError::InvalidRoot)
    ));
    assert!(
        fs::symlink_metadata(&link)
            .expect("link remains")
            .file_type()
            .is_symlink()
    );

    fs::remove_file(link).expect("cleanup link");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[cfg(unix)]
#[test]
fn creating_a_nested_store_rejects_a_substituted_missing_component() {
    use std::os::unix::fs::symlink;

    let parent = temporary_directory("artifact-create-retained-parent");
    let outside = temporary_directory("artifact-create-retained-outside");
    fs::create_dir_all(&parent).expect("retained parent");
    fs::create_dir_all(&outside).expect("outside");
    symlink(&outside, parent.join("substituted")).expect("substituted component");

    assert!(matches!(
        ArtifactStore::open(parent.join("substituted/store")),
        Err(ArtifactError::InvalidRoot)
    ));
    assert!(!outside.join("store").exists());

    fs::remove_dir_all(parent).expect("cleanup parent");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[cfg(unix)]
#[test]
fn artifact_load_rejects_a_symlink_substitution() {
    use std::os::unix::fs::symlink;

    let root = temporary_directory("artifact-symlink");
    let outside = temporary_directory("artifact-outside");
    fs::create_dir_all(&outside).expect("outside");
    let store = ArtifactStore::open(&root).expect("store");
    let reference = store
        .put(b"trusted", ContentSensitivity::Portable)
        .expect("put");
    let path = fs::read_dir(&root)
        .expect("artifacts")
        .next()
        .expect("artifact")
        .expect("entry")
        .path();
    fs::remove_file(&path).expect("remove artifact");
    let outside_file = outside.join("outside");
    fs::write(&outside_file, b"trusted").expect("outside file");
    symlink(outside_file, path).expect("substitute symlink");
    assert!(matches!(
        store.load(&reference),
        Err(ArtifactError::UnsafeArtifactType)
    ));
    fs::remove_dir_all(root).expect("cleanup root");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[cfg(unix)]
#[test]
fn an_opened_existing_store_remains_bound_to_the_original_directory() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temporary_directory("artifact-root-race");
    let moved = temporary_directory("artifact-root-original");
    let substitute = temporary_directory("artifact-root-substitute");
    let creating_store = ArtifactStore::open(&root).expect("create store");
    let reference = creating_store
        .put(b"original trusted bytes", ContentSensitivity::Portable)
        .expect("put original");
    drop(creating_store);

    let store = ArtifactStore::open_existing(&root).expect("open existing capability");
    fs::rename(&root, &moved).expect("move opened store");
    fs::create_dir_all(&substitute).expect("substitute root");
    fs::set_permissions(&substitute, fs::Permissions::from_mode(0o700))
        .expect("private substitute");
    let blob_name = format!(
        "{}.blob",
        reference.digest.as_str().trim_start_matches("sha256:")
    );
    fs::write(substitute.join(blob_name), b"substituted malicious bytes").expect("substitute blob");
    symlink(&substitute, &root).expect("replace path with symlink");

    assert_eq!(
        store
            .load(&reference)
            .expect("load through retained handle"),
        b"original trusted bytes"
    );

    fs::remove_file(root).expect("cleanup link");
    fs::remove_dir_all(moved).expect("cleanup original");
    fs::remove_dir_all(substitute).expect("cleanup substitute");
}

#[cfg(unix)]
#[test]
fn creating_stores_during_ancestor_substitution_never_creates_outside() {
    use std::os::unix::fs::symlink;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let base = temporary_directory("artifact-concurrent-create-base");
    let ancestor = base.join("ancestor");
    let parked = base.join("parked");
    let outside = temporary_directory("artifact-concurrent-create-outside");
    fs::create_dir_all(&ancestor).expect("ancestor");
    fs::create_dir_all(&outside).expect("outside");

    let stop = Arc::new(AtomicBool::new(false));
    let attacker_stop = Arc::clone(&stop);
    let attacker_ancestor = ancestor.clone();
    let attacker_parked = parked.clone();
    let attacker_outside = outside.clone();
    let attacker = std::thread::spawn(move || {
        while !attacker_stop.load(Ordering::Acquire) {
            if fs::rename(&attacker_ancestor, &attacker_parked).is_ok() {
                if symlink(&attacker_outside, &attacker_ancestor).is_ok() {
                    std::thread::yield_now();
                    let _ = fs::remove_file(&attacker_ancestor);
                }
                let _ = fs::rename(&attacker_parked, &attacker_ancestor);
            } else {
                std::thread::yield_now();
            }
        }
        if attacker_ancestor
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            let _ = fs::remove_file(&attacker_ancestor);
        }
        if attacker_parked.exists() {
            let _ = fs::rename(&attacker_parked, &attacker_ancestor);
        }
    });

    for index in 0..2_000 {
        let _ = ArtifactStore::open(ancestor.join(format!("store-{index}")));
    }
    stop.store(true, Ordering::Release);
    attacker.join().expect("attacker");

    assert_eq!(
        fs::read_dir(&outside).expect("outside").count(),
        0,
        "artifact bootstrap escaped through a concurrently substituted ancestor"
    );
    fs::remove_dir_all(base).expect("cleanup base");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

#[cfg(unix)]
#[test]
fn opening_existing_stores_during_ancestor_substitution_never_opens_outside() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let base = temporary_directory("artifact-concurrent-open-base");
    let ancestor = base.join("ancestor");
    let parked = base.join("parked");
    let outside = temporary_directory("artifact-concurrent-open-outside");
    let trusted_store = ancestor.join("store");
    let outside_store = outside.join("store");
    let trusted = ArtifactStore::open(&trusted_store).expect("trusted store");
    let reference = trusted
        .put(b"trusted bytes", ContentSensitivity::Portable)
        .expect("trusted artifact");
    drop(trusted);
    fs::create_dir_all(&outside_store).expect("outside store");
    fs::set_permissions(&outside_store, fs::Permissions::from_mode(0o700))
        .expect("outside store mode");
    let blob = format!(
        "{}.blob",
        reference.digest.as_str().trim_start_matches("sha256:")
    );
    fs::write(outside_store.join(blob), b"outside bytes").expect("outside artifact");

    let stop = Arc::new(AtomicBool::new(false));
    let attacker_stop = Arc::clone(&stop);
    let attacker_ancestor = ancestor.clone();
    let attacker_parked = parked.clone();
    let attacker_outside = outside.clone();
    let attacker = std::thread::spawn(move || {
        while !attacker_stop.load(Ordering::Acquire) {
            if fs::rename(&attacker_ancestor, &attacker_parked).is_ok() {
                if symlink(&attacker_outside, &attacker_ancestor).is_ok() {
                    std::thread::yield_now();
                    let _ = fs::remove_file(&attacker_ancestor);
                }
                let _ = fs::rename(&attacker_parked, &attacker_ancestor);
            } else {
                std::thread::yield_now();
            }
        }
        if attacker_ancestor
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            let _ = fs::remove_file(&attacker_ancestor);
        }
        if attacker_parked.exists() {
            let _ = fs::rename(&attacker_parked, &attacker_ancestor);
        }
    });

    for _ in 0..2_000 {
        if let Ok(store) = ArtifactStore::open_existing(&trusted_store) {
            assert_eq!(
                store.load(&reference).expect("retained trusted store"),
                b"trusted bytes"
            );
        }
    }
    stop.store(true, Ordering::Release);
    attacker.join().expect("attacker");

    fs::remove_dir_all(base).expect("cleanup base");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

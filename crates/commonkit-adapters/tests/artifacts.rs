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

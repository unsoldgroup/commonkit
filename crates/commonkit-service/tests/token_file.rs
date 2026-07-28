use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_service::ControlToken;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "commonkit-token-{}-{nonce}-{}",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(&path).expect("token test directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("private token test directory");
        assert_eq!(
            fs::metadata(&path)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "control-token fixtures must not rely on a permissive temporary parent"
        );
    }
    path
}

#[test]
fn creates_and_reloads_the_same_private_token() {
    let directory = temporary_directory();
    let path = directory.join("control.token");
    let first = ControlToken::load_or_create(&path).expect("create");
    let second = ControlToken::load_or_create(&path).expect("reload");
    assert_eq!(first.expose_for_client(), second.expose_for_client());
    assert_eq!(first.expose_for_client().len(), 64);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(directory).expect("cleanup");
}

#[cfg(unix)]
#[test]
fn rejects_overbroad_token_permissions_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = temporary_directory();
    let path = directory.join("control.token");
    fs::write(&path, "0".repeat(64)).expect("token");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("permissions");
    assert!(ControlToken::load_or_create(&path).is_err());

    let link = directory.join("control-link.token");
    symlink(&path, &link).expect("symlink");
    assert!(ControlToken::load_or_create(&link).is_err());
    fs::remove_dir_all(directory).expect("cleanup");
}

#[cfg(windows)]
#[test]
fn rejects_token_with_broad_explicit_windows_ace() {
    let directory = temporary_directory();
    let path = directory.join("control.token");
    ControlToken::load_or_create(&path).expect("create private token");

    let status = std::process::Command::new("icacls.exe")
        .arg(&path)
        .args(["/grant", "*S-1-1-0:(R)"])
        .status()
        .expect("run icacls");
    assert!(status.success(), "fixture must add a broad explicit ACE");

    assert!(ControlToken::load(&path).is_err());
    fs::remove_dir_all(directory).expect("cleanup");
}

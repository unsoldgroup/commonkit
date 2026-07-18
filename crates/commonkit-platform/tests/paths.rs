use std::path::Path;

use commonkit_platform::{AppPaths, HostPlatform, PrivatePathKind, SecurityCapability};

#[test]
fn derives_separate_private_state_domains() {
    let paths = AppPaths::from_roots(
        Path::new("/tmp/commonkit-test/config"),
        Path::new("/tmp/commonkit-test/data"),
        Path::new("/tmp/commonkit-test/cache"),
    )
    .expect("paths");
    assert_eq!(paths.receipts, paths.state.join("receipts"));
    assert_eq!(paths.plans, paths.state.join("plans"));
    assert_eq!(paths.backups, paths.state.join("backups"));
    assert_eq!(paths.snapshots, paths.state.join("snapshots"));
    assert_ne!(paths.config, paths.state);
    assert_ne!(paths.cache, paths.state);
}

#[test]
fn rejects_relative_filesystem_root_and_overlapping_locations() {
    assert!(AppPaths::from_roots("relative", "/tmp/data", "/tmp/cache").is_err());
    assert!(AppPaths::from_roots("/", "/tmp/data", "/tmp/cache").is_err());
    assert!(AppPaths::from_roots("/tmp/data/state", "/tmp/data", "/tmp/cache").is_err());
}

#[test]
fn declares_fail_closed_security_capabilities_for_every_supported_platform() {
    let mac = HostPlatform::MacOs.security_capabilities();
    assert!(mac.contains(&SecurityCapability::PosixModes));
    assert!(mac.contains(&SecurityCapability::Keychain));
    assert!(mac.contains(&SecurityCapability::ProcessSandbox));

    let linux = HostPlatform::Linux.security_capabilities();
    assert!(linux.contains(&SecurityCapability::PosixModes));
    assert!(linux.contains(&SecurityCapability::SecretService));
    assert!(linux.contains(&SecurityCapability::ProcessSandbox));

    let windows = HostPlatform::Windows.security_capabilities();
    assert!(windows.contains(&SecurityCapability::WindowsAcl));
    assert!(windows.contains(&SecurityCapability::CredentialManager));
    assert!(windows.contains(&SecurityCapability::ProcessSandbox));
}

#[test]
fn windows_acl_audit_rejects_inherited_and_broad_grants() {
    use commonkit_platform::{windows_acl_listing_is_private, windows_private_acl_args};
    assert_eq!(
        windows_private_acl_args("alice").unwrap(),
        [
            "/inheritance:r",
            "/remove:g",
            "*S-1-1-0",
            "*S-1-5-11",
            "*S-1-5-32-545",
            "*S-1-15-2-1",
            "/grant:r",
            "alice:(F)"
        ]
    );
    assert!(windows_private_acl_args("alice\nEveryone:(F)").is_err());
    assert!(windows_acl_listing_is_private(
        r"C:\state DESKTOP\alice:(F)\nSuccessfully processed 1 files",
        "alice"
    ));
    assert!(!windows_acl_listing_is_private(
        r"C:\state DESKTOP\alice:(F) BUILTIN\Users:(RX)",
        "alice"
    ));
    assert!(!windows_acl_listing_is_private(
        r"C:\state DESKTOP\alice:(I)(F)",
        "alice"
    ));
    assert!(!windows_acl_listing_is_private(
        r"C:\state NT AUTHORITY\SYSTEM:(F)",
        "alice"
    ));
}

#[cfg(windows)]
#[test]
fn native_windows_private_path_acl_is_enforced_and_verified() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("private-token");
    commonkit_platform::ensure_private_path(&file, PrivatePathKind::File).unwrap();
    commonkit_platform::verify_private_path(&file, PrivatePathKind::File).unwrap();
}

#[cfg(unix)]
#[test]
fn private_paths_are_created_without_following_symlinks_and_verified() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("private");
    commonkit_platform::ensure_private_path(&directory, PrivatePathKind::Directory).unwrap();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    commonkit_platform::verify_private_path(&directory, PrivatePathKind::Directory).unwrap();

    let file = directory.join("token");
    commonkit_platform::ensure_private_path(&file, PrivatePathKind::File).unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(commonkit_platform::verify_private_path(&link, PrivatePathKind::File).is_err());
}

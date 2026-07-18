use std::path::Path;

use commonkit_platform::AppPaths;

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

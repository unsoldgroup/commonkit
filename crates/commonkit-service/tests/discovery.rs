use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_service::{ControlToken, DaemonDiscovery};

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "commonkit-discovery-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn persists_discovery_without_the_bearer_capability() {
    let directory = temporary_directory();
    let path = directory.join("daemon.json");
    let token = ControlToken::generate();
    let discovery = DaemonDiscovery::new(3764, &token).expect("discovery");
    discovery.persist(&path).expect("persist");

    let bytes = fs::read(&path).expect("read");
    let loaded: DaemonDiscovery = serde_json::from_slice(&bytes).expect("parse");
    assert_eq!(loaded, discovery);
    assert_eq!(loaded.port, 3764);
    assert!(!String::from_utf8_lossy(&bytes).contains(token.expose_for_client()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(directory).expect("cleanup");
}

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use commonkit_service::{BoundServer, ControlToken, EventHub, ServiceStatus};
use tokio::sync::{RwLock, oneshot};

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("commonkit-server-{}-{nonce}", std::process::id()))
}

#[tokio::test]
async fn publishes_discovery_only_after_binding_and_removes_it_on_shutdown() {
    let directory = temporary_directory();
    let discovery_path = directory.join("daemon.json");
    let server = BoundServer::bind(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        ControlToken::generate(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(8),
        &discovery_path,
    )
    .await
    .expect("bind");
    assert!(server.discovery().port > 0);
    assert!(discovery_path.exists());

    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(server.run_until(async move {
        let _ = signal.await;
    }));
    shutdown.send(()).expect("shutdown");
    task.await.expect("task").expect("server");
    assert!(!discovery_path.exists());
    std::fs::remove_dir_all(directory).expect("cleanup");
}

#[tokio::test]
async fn rejects_non_loopback_binding() {
    let directory = temporary_directory();
    let result = BoundServer::bind(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        ControlToken::generate(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(8),
        directory.join("daemon.json"),
    )
    .await;
    assert!(result.is_err());
}

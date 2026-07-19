use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use commonkit_service::{BoundServer, ControlToken, EventHub, ServiceStatus};
use std::io::{Read, Write};
use tokio::sync::{RwLock, oneshot};

fn temporary_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("commonkit-server-")
        .tempdir()
        .expect("unique server test directory")
}

#[tokio::test]
async fn publishes_discovery_only_after_binding_and_removes_it_on_shutdown() {
    let directory = temporary_directory();
    let discovery_path = directory.path().join("daemon.json");
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
}

#[tokio::test]
async fn rejects_non_loopback_binding() {
    let directory = temporary_directory();
    let result = BoundServer::bind(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        ControlToken::generate(),
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(8),
        directory.path().join("daemon.json"),
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn hosts_a_separate_authenticated_loopback_mcp_listener() {
    let directory = temporary_directory();
    let token = ControlToken::generate();
    let bearer = token.expose_for_client().to_owned();
    let server = BoundServer::bind_with_relay_address(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        token,
        Arc::new(RwLock::new(ServiceStatus::default())),
        EventHub::new(8),
        directory.path().join("daemon.json"),
    )
    .await
    .expect("bind");
    let relay = server.relay_address().expect("relay address");
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(server.run_until(async move {
        let _ = signal.await;
    }));
    let tools_bearer = bearer.clone();
    let response = tokio::task::spawn_blocking(move || {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let mut stream = std::net::TcpStream::connect(relay).expect("connect relay");
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {tools_bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }).await.expect("request task");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(response.contains("relay_unconfigured"));
    let initialized = tokio::task::spawn_blocking(move || {
        let body = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let mut stream = std::net::TcpStream::connect(relay).expect("connect relay");
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }).await.expect("initialize task");
    assert!(initialized.starts_with("HTTP/1.1 200"), "{initialized}");
    assert!(initialized.contains("commonkit-relay"));
    shutdown.send(()).expect("shutdown");
    task.await.expect("task").expect("server");
}

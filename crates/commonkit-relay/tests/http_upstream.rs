use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::fs;

use commonkit_relay::{HttpUpstreamManager, RelayConfig, RelayHealth, UpstreamManager};
use serde_json::json;

#[test]
fn bounded_http_upstream_discovers_tools_and_tracks_health() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 4096];
        let length = stream.read(&mut request).expect("request");
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.contains("tools/list"));
        let body = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": "commonkit-tools",
            "result": {"tools": [{"name": "search", "description": "Search", "inputSchema": {"type": "object"}}]}
        })).expect("body");
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).expect("response");
    });
    let mut server = RelayConfig::normalize(json!({"servers": [{
        "id": "local", "remote": {"type": "streamable_http", "url": format!("http://{address}/mcp")}
    }]}))
    .expect("config")
    .servers
    .remove(0);
    server.remote.url = format!("http://{address}/mcp");
    let manager = HttpUpstreamManager::new(Duration::from_secs(2)).expect("manager");
    let tools = manager.discover(&server).expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "search");
    assert_eq!(manager.health(&server), RelayHealth::Healthy);
    server_thread.join().expect("server thread");
}

#[test]
fn http_upstream_rejects_unbounded_timeout_configuration() {
    assert!(HttpUpstreamManager::new(Duration::from_secs(121)).is_err());
}

#[cfg(unix)]
#[test]
fn http_upstream_resolves_headers_from_the_servers_private_env_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("root");
    let env_path = root.path().join("docs.env");
    fs::write(&env_path, "DOCS_TOKEN='Bearer private-value'\n").expect("env");
    fs::set_permissions(&env_path, fs::Permissions::from_mode(0o600)).expect("permissions");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 4096];
        let length = stream.read(&mut request).expect("request");
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer private-value\r\n")
        );
        let body = r#"{"jsonrpc":"2.0","id":"commonkit-tools","result":{"tools":[]}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("response");
    });
    let mut server = RelayConfig::normalize(json!({"servers": [{
        "id": "docs",
        "envFile": "docs.env",
        "remote": {
            "type": "streamable_http",
            "url": format!("http://{address}/mcp"),
            "headers": {"Authorization": "env:DOCS_TOKEN"}
        }
    }]}))
    .expect("config")
    .servers
    .remove(0);
    server.remote.url = format!("http://{address}/mcp");
    let manager =
        HttpUpstreamManager::with_env_root(Duration::from_secs(2), root.path()).expect("manager");
    assert!(manager.discover(&server).is_ok());
    server_thread.join().expect("server");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_upstream_manager_can_be_created_and_dropped_inside_tokio() {
    let manager = HttpUpstreamManager::new(Duration::from_secs(1)).expect("manager");
    drop(manager);
}

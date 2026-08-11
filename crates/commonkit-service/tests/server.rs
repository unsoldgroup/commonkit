use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use commonkit_adapters::{
    ArtifactStore, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, ProviderCapability, ProviderCapabilityResource, ProviderInputs,
    ResourceProvenance, materialize_mcp_client_state,
};
use commonkit_contracts::{Sha256Digest, StableId};
use std::collections::BTreeMap;

use commonkit_service::{BoundServer, ControlToken, EventHub, ServiceStatus};
use std::io::{Read, Write};
use tokio::sync::{RwLock, oneshot};

fn temporary_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("commonkit-server-")
        .tempdir()
        .expect("unique server test directory")
}

fn generated_relay_authorization(
    directory: &tempfile::TempDir,
    endpoint: &str,
    token: &str,
) -> String {
    let inputs = ProviderInputs::new(
        StableId::parse("apm").unwrap(),
        ExactProviderVersion::parse("0.25.0").unwrap(),
        "commonkit.apm-provider.v1".into(),
        BTreeMap::from([(
            "manifest".into(),
            Sha256Digest::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
        )]),
        vec!["agent-context".into()],
    )
    .unwrap();
    let provider = MaterializedState::finalize_with_capabilities(
        inputs.clone(),
        vec![],
        vec![],
        vec![],
        vec![ProviderCapabilityResource {
            capability: ProviderCapability::McpStreamableHttp {
                id: "docs".into(),
                name: "Docs".into(),
                enabled: true,
                url: "https://upstream.example/mcp".into(),
                headers: BTreeMap::new(),
            },
            provenance: ResourceProvenance {
                provider_id: inputs.provider_id.clone(),
                provider_version: inputs.provider_version.to_string(),
                input_digest: inputs.input_set_digest.clone(),
                source: "apm.yml:dependencies.mcp[docs]".into(),
            },
        }],
    )
    .unwrap();
    let artifacts = ArtifactStore::open(directory.path().join("artifacts")).unwrap();
    let generated = materialize_mcp_client_state(
        &[provider],
        &artifacts,
        &NormalizedManagedPath::parse("home").unwrap(),
        endpoint,
    )
    .unwrap()
    .unwrap();
    let files = generated
        .resources
        .iter()
        .map(|resource| {
            let Some(FilesystemIntent::File { path, content, .. }) = resource.intent.filesystem()
            else {
                panic!("client resource must be a file")
            };
            (path.as_str(), artifacts.load(content).unwrap())
        })
        .collect::<BTreeMap<_, _>>();
    let claude: serde_json::Value =
        serde_json::from_slice(files["home/.mcp.json"].as_slice()).unwrap();
    assert_eq!(
        claude["mcpServers"]["commonkit-relay"]["command"],
        "commonkit"
    );
    assert_eq!(
        claude["mcpServers"]["commonkit-relay"]["args"],
        serde_json::json!(["relay-client"])
    );
    let codex = String::from_utf8(files["home/.codex/config.toml"].clone()).unwrap();
    assert!(codex.contains("command = \"commonkit\""));
    assert!(codex.contains("args = [\"relay-client\"]"));
    format!("Bearer {token}")
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
    let endpoint = format!("http://{relay}/mcp");
    let authorization = generated_relay_authorization(&directory, &endpoint, &bearer);
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(server.run_until(async move {
        let _ = signal.await;
    }));
    let tools_authorization = authorization.clone();
    let response = tokio::task::spawn_blocking(move || {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let mut stream = std::net::TcpStream::connect(relay).expect("connect relay");
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {tools_authorization}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }).await.expect("request task");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(response.contains("relay_unconfigured"));
    let initialized = tokio::task::spawn_blocking(move || {
        let body = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let mut stream = std::net::TcpStream::connect(relay).expect("connect relay");
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {authorization}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }).await.expect("initialize task");
    assert!(initialized.starts_with("HTTP/1.1 200"), "{initialized}");
    assert!(initialized.contains("commonkit-relay"));
    shutdown.send(()).expect("shutdown");
    task.await.expect("task").expect("server");
}

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

use commonkit_adapters::{
    ArtifactStore, ExactProviderVersion, FilesystemIntent, MaterializedState,
    NormalizedManagedPath, ProviderCapability, ProviderCapabilityResource, ProviderInputs,
    ResourceProvenance, materialize_mcp_client_state,
};
use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_service::{ControlToken, DaemonDiscovery};

fn isolate(command: &mut Command, root: &std::path::Path) {
    command
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("local-appdata"))
        .env_remove("COMMONKIT_RELAY_TOKEN");
}

fn generated_command(root: &std::path::Path) -> (String, Vec<String>) {
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
    let artifacts = ArtifactStore::open(root.join("artifacts")).unwrap();
    let state = materialize_mcp_client_state(
        &[provider],
        &artifacts,
        &NormalizedManagedPath::parse("home").unwrap(),
        "http://127.0.0.1:3764/mcp",
    )
    .unwrap()
    .unwrap();
    let resource = state
        .resources
        .iter()
        .find(|resource| resource.intent.path().as_str() == "home/.mcp.json")
        .unwrap();
    let FilesystemIntent::File { content, .. } = &resource.intent else {
        panic!("generated client must be a file")
    };
    let value: serde_json::Value =
        serde_json::from_slice(&artifacts.load(content).unwrap()).unwrap();
    let server = &value["mcpServers"]["commonkit-relay"];
    (
        server["command"].as_str().unwrap().to_owned(),
        server["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect(),
    )
}

#[test]
fn generated_mcp_stdio_bridge_authenticates_without_an_environment_token() {
    let root = tempfile::tempdir().unwrap();
    let mut status = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate(&mut status, root.path());
    let status: serde_json::Value =
        serde_json::from_slice(&status.arg("status").output().unwrap().stdout).unwrap();
    let config = std::path::PathBuf::from(status["configDirectory"].as_str().unwrap());
    let state = std::path::PathBuf::from(status["stateDirectory"].as_str().unwrap());
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let token = ControlToken::load_or_create(&config.join("control.token")).unwrap();
    let expected_authorization = format!("authorization: bearer {}", token.expose_for_client());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let discovery = DaemonDiscovery::new(1, Some(port), &token).unwrap();
    std::fs::write(
        state.join("daemon.json"),
        serde_json::to_vec(&discovery).unwrap(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]).to_ascii_lowercase();
        assert!(request.contains(&expected_authorization));
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"commonkit-relay"}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let (program, args) = generated_command(root.path());
    assert_eq!(program, "commonkit");
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonkit"));
    isolate(&mut command, root.path());
    let mut child = command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("commonkit-relay"));
    server.join().unwrap();
}

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use commonkit_relay::{
    DownstreamRequest, PeerAddress, RelayConfig, RelayErrorCode, RelayHealth, RelayRuntime,
    RelayServerConfig, RelayServerId, RelayTool, RuntimeNotification, UpstreamError,
    UpstreamManager,
};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeUpstreams {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    tools: BTreeMap<String, Vec<RelayTool>>,
    calls: Vec<(String, String, Value)>,
    discover_error: Option<String>,
    call_error: Option<String>,
    healthy: bool,
    reconnects: usize,
}

impl UpstreamManager for FakeUpstreams {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, UpstreamError> {
        let state = self.state.lock().unwrap();
        if let Some(message) = &state.discover_error {
            return Err(UpstreamError::Unavailable(message.clone()));
        }
        Ok(state
            .tools
            .get(server.id.as_str())
            .cloned()
            .unwrap_or_default())
    }

    fn call(
        &self,
        server: &RelayServerConfig,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, UpstreamError> {
        let mut state = self.state.lock().unwrap();
        if let Some(message) = &state.call_error {
            return Err(UpstreamError::Unavailable(message.clone()));
        }
        state
            .calls
            .push((server.id.as_str().into(), tool.into(), arguments.clone()));
        Ok(json!({"server": server.id.as_str(), "tool": tool, "arguments": arguments}))
    }

    fn health(&self, _server: &RelayServerConfig) -> RelayHealth {
        if self.state.lock().unwrap().healthy {
            RelayHealth::Healthy
        } else {
            RelayHealth::Unavailable
        }
    }

    fn reconnect(&self, _server: &RelayServerConfig) -> Result<(), UpstreamError> {
        let mut state = self.state.lock().unwrap();
        state.reconnects += 1;
        state.healthy = true;
        Ok(())
    }
}

fn server(id: &str, refresh_ms: u64) -> RelayServerConfig {
    let mut server = RelayConfig::normalize(json!({"servers": [{
        "id": id,
        "remote": {"type": "streamable_http", "url": format!("https://{id}.example/mcp")},
        "cache": {"toolsTtlMs": 100, "autoRefreshMs": refresh_ms}
    }]}))
    .unwrap()
    .servers
    .remove(0);
    server.cache.auto_refresh_ms = refresh_ms;
    server
}

fn tool(name: &str) -> RelayTool {
    RelayTool {
        name: name.into(),
        description: String::new(),
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn downstream_is_loopback_only_and_bearer_authenticated() {
    let upstreams = FakeUpstreams::default();
    upstreams
        .state
        .lock()
        .unwrap()
        .tools
        .insert("docs".into(), vec![tool("search")]);
    let runtime =
        RelayRuntime::new("private-token", upstreams, vec![server("docs", 100)], 0).unwrap();

    let remote = runtime.handle(
        PeerAddress::Remote,
        Some("Bearer private-token"),
        DownstreamRequest::ListTools,
    );
    assert_eq!(remote.unwrap_err().code(), RelayErrorCode::Forbidden);
    let missing = runtime.handle(PeerAddress::Loopback, None, DownstreamRequest::ListTools);
    assert_eq!(missing.unwrap_err().code(), RelayErrorCode::Unauthorized);
    let tools = runtime
        .handle(
            PeerAddress::Loopback,
            Some("Bearer private-token"),
            DownstreamRequest::ListTools,
        )
        .unwrap();
    assert_eq!(
        tools,
        json!({"tools":[{"name":"docs__search","description":"[docs] search","inputSchema":{"type":"object"}}]})
    );
}

#[test]
fn forwards_localized_calls_and_bounds_and_redacts_upstream_errors() {
    let upstreams = FakeUpstreams::default();
    upstreams
        .state
        .lock()
        .unwrap()
        .tools
        .insert("docs".into(), vec![tool("search")]);
    let runtime =
        RelayRuntime::new("token", upstreams.clone(), vec![server("docs", 100)], 0).unwrap();
    let result = runtime
        .handle(
            PeerAddress::Loopback,
            Some("Bearer token"),
            DownstreamRequest::CallTool {
                name: "docs__search".into(),
                arguments: json!({"q":"rust"}),
            },
        )
        .unwrap();
    assert_eq!(result["server"], "docs");
    assert_eq!(upstreams.state.lock().unwrap().calls[0].1, "search");

    upstreams.state.lock().unwrap().call_error =
        Some(format!("secret=do-not-leak {}", "x".repeat(1000)));
    let error = runtime
        .handle(
            PeerAddress::Loopback,
            Some("Bearer token"),
            DownstreamRequest::CallTool {
                name: "docs__search".into(),
                arguments: json!({}),
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), RelayErrorCode::UpstreamUnavailable);
    assert!(!error.to_string().contains("do-not-leak"));
    assert!(error.to_string().len() <= 160);
}

#[test]
fn refresh_retains_stale_cache_notifies_on_change_and_reconnects_unhealthy_upstream() {
    let upstreams = FakeUpstreams::default();
    upstreams
        .state
        .lock()
        .unwrap()
        .tools
        .insert("docs".into(), vec![tool("old")]);
    let mut runtime =
        RelayRuntime::new("token", upstreams.clone(), vec![server("docs", 10)], 0).unwrap();
    runtime.take_notifications();

    upstreams.state.lock().unwrap().discover_error = Some("Bearer private and more".into());
    runtime.refresh_due(11);
    assert_eq!(runtime.list_tools().unwrap()[0].name, "docs__old");
    assert_eq!(
        runtime.health(&RelayServerId::parse("docs").unwrap()),
        Some(RelayHealth::Healthy)
    );
    assert_eq!(upstreams.state.lock().unwrap().reconnects, 1);

    {
        let mut state = upstreams.state.lock().unwrap();
        state.discover_error = None;
        state.tools.insert("docs".into(), vec![tool("new")]);
    }
    runtime.refresh_due(22);
    assert_eq!(runtime.list_tools().unwrap()[0].name, "docs__new");
    assert_eq!(
        runtime.take_notifications(),
        vec![RuntimeNotification::ToolsListChanged]
    );
}

#[test]
fn reconcile_is_atomic_when_any_new_upstream_fails_discovery() {
    let upstreams = FakeUpstreams::default();
    upstreams
        .state
        .lock()
        .unwrap()
        .tools
        .insert("old".into(), vec![tool("old")]);
    let mut runtime =
        RelayRuntime::new("token", upstreams.clone(), vec![server("old", 10)], 0).unwrap();
    upstreams.state.lock().unwrap().discover_error = Some("failure".into());
    assert!(runtime.reconcile(vec![server("new", 10)], true, 1).is_err());
    assert_eq!(runtime.list_tools().unwrap()[0].name, "old__old");
}

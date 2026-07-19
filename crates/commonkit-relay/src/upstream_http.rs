use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Response};
use serde_json::{Value, json};

use crate::{RelayHealth, RelayServerConfig, RelayTool, UpstreamError, UpstreamManager};

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Bounded Streamable HTTP client for persistent relay upstreams.
///
/// Credential references are resolved only while constructing request headers;
/// values and upstream response bodies never cross the redacted error boundary.
pub struct HttpUpstreamManager {
    client: Client,
    health: Mutex<BTreeMap<String, RelayHealth>>,
}

impl HttpUpstreamManager {
    pub fn new(timeout: Duration) -> Result<Self, UpstreamError> {
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(unavailable());
        }
        let client = Client::builder()
            .connect_timeout(timeout.min(Duration::from_secs(15)))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            client,
            health: Mutex::new(BTreeMap::new()),
        })
    }

    fn request(&self, server: &RelayServerConfig, body: Value) -> Result<Value, UpstreamError> {
        let headers = resolve_headers(server)?;
        let client = self.client.clone();
        let url = server.remote.url.clone();
        let value = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| unavailable())?;
            runtime.block_on(async move {
                let response = client
                    .post(url)
                    .headers(headers)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|_| unavailable())?;
                decode_bounded(response).await
            })
        })
        .join()
        .map_err(|_| self.failed(server))?
        .map_err(|_| self.failed(server))?;
        self.health
            .lock()
            .expect("relay health lock")
            .insert(server.id.as_str().into(), RelayHealth::Healthy);
        if value.get("error").is_some() {
            return Err(self.failed(server));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    fn failed(&self, server: &RelayServerConfig) -> UpstreamError {
        self.health
            .lock()
            .expect("relay health lock")
            .insert(server.id.as_str().into(), RelayHealth::Unavailable);
        unavailable()
    }
}

impl UpstreamManager for HttpUpstreamManager {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, UpstreamError> {
        let result = self.request(
            server,
            json!({
                "jsonrpc": "2.0", "id": "commonkit-tools", "method": "tools/list", "params": {}
            }),
        )?;
        serde_json::from_value(result.get("tools").cloned().unwrap_or(Value::Array(vec![])))
            .map_err(|_| self.failed(server))
    }

    fn call(
        &self,
        server: &RelayServerConfig,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, UpstreamError> {
        self.request(
            server,
            json!({
                "jsonrpc": "2.0", "id": "commonkit-call", "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            }),
        )
    }

    fn health(&self, server: &RelayServerConfig) -> RelayHealth {
        self.health
            .lock()
            .expect("relay health lock")
            .get(server.id.as_str())
            .copied()
            .unwrap_or(RelayHealth::Unavailable)
    }

    fn reconnect(&self, server: &RelayServerConfig) -> Result<(), UpstreamError> {
        self.discover(server).map(|_| ())
    }
}

fn resolve_headers(server: &RelayServerConfig) -> Result<HeaderMap, UpstreamError> {
    let mut headers = HeaderMap::new();
    for (name, reference) in &server.remote.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| unavailable())?;
        let variable = reference
            .strip_prefix("env:")
            .or_else(|| reference.strip_prefix("secret:"))
            .ok_or_else(unavailable)?;
        let value = std::env::var(variable).map_err(|_| unavailable())?;
        headers.insert(
            name,
            HeaderValue::from_str(&value).map_err(|_| unavailable())?,
        );
    }
    Ok(headers)
}

async fn decode_bounded(mut response: Response) -> Result<Value, UpstreamError> {
    if !response.status().is_success() {
        return Err(unavailable());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) as u64 > MAX_RESPONSE_BYTES {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

fn unavailable() -> UpstreamError {
    UpstreamError::Unavailable("upstream unavailable".into())
}

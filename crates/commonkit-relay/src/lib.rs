//! Rust MCP relay configuration and atomic desired-state lifecycle.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

pub const DEFAULT_PORT: u16 = 3764;
pub const DEFAULT_MCP_PATH: &str = "/mcp";
pub const DEFAULT_TOOLS_TTL_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayServerId(String);

impl RelayServerId {
    pub fn parse(value: impl Into<String>) -> Result<Self, RelayConfigError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 63
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if valid {
            Ok(Self(value))
        } else {
            Err(RelayConfigError::InvalidServerId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelayMode {
    GenericCached,
    PosthogCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedBy {
    CommonKit,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayListenConfig {
    pub host: String,
    pub port: u16,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayCacheConfig {
    pub tools_ttl_ms: u64,
    pub auto_refresh_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayRemoteConfig {
    #[serde(rename = "type")]
    pub transport_type: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayMenuAction {
    pub id: RelayServerId,
    pub label: String,
    pub confirm: bool,
    pub tool: Option<String>,
    pub args: Option<Value>,
    pub url: Option<String>,
    pub method: Option<String>,
    pub view: Option<Value>,
    pub input: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayMenuConfig {
    pub status_url: Option<String>,
    pub ttl_ms: u64,
    pub actions: Vec<RelayMenuAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayServerConfig {
    pub id: RelayServerId,
    pub name: String,
    pub description: String,
    pub category: String,
    pub enabled: bool,
    pub mode: RelayMode,
    pub remote: RelayRemoteConfig,
    pub env_file: Option<String>,
    pub cache: RelayCacheConfig,
    pub menu: Option<RelayMenuConfig>,
    pub managed_by: ManagedBy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConfig {
    pub listen: RelayListenConfig,
    pub servers: Vec<RelayServerConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelayRuntimeServer {
    pub config: RelayServerConfig,
    pub tools: Vec<RelayTool>,
}

pub trait UpstreamValidator {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, RelayLifecycleError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileSummary {
    pub added: Vec<RelayServerId>,
    pub updated: Vec<RelayServerId>,
    pub removed: Vec<RelayServerId>,
}

#[derive(Debug, Clone, Default)]
pub struct RelayCatalog {
    servers: BTreeMap<RelayServerId, RelayRuntimeServer>,
}

impl RelayCatalog {
    pub fn from_servers(servers: Vec<RelayRuntimeServer>) -> Result<Self, RelayLifecycleError> {
        let mut catalog = Self::default();
        for server in servers {
            if catalog
                .servers
                .insert(server.config.id.clone(), server)
                .is_some()
            {
                return Err(RelayLifecycleError::DuplicateServer);
            }
        }
        Ok(catalog)
    }

    pub fn servers(&self) -> impl Iterator<Item = &RelayRuntimeServer> {
        self.servers.values()
    }

    pub fn localized_tools(&self) -> Result<Vec<RelayTool>, RelayLifecycleError> {
        let mut names = BTreeSet::new();
        let mut tools = Vec::new();
        for server in self.servers.values().filter(|server| server.config.enabled) {
            for tool in &server.tools {
                let localized = localize_tool(server.config.id.as_str(), tool);
                if !names.insert(localized.name.clone()) {
                    return Err(RelayLifecycleError::ToolNameCollision(localized.name));
                }
                tools.push(localized);
            }
        }
        Ok(tools)
    }

    pub fn reconcile(
        &mut self,
        desired: Vec<RelayServerConfig>,
        prune_commonkit: bool,
        validator: &impl UpstreamValidator,
    ) -> Result<ReconcileSummary, RelayLifecycleError> {
        let mut desired_ids = BTreeSet::new();
        let mut prepared = Vec::new();
        for server in desired {
            if !desired_ids.insert(server.id.clone()) {
                return Err(RelayLifecycleError::DuplicateServer);
            }
            if self
                .servers
                .get(&server.id)
                .is_some_and(|existing| existing.config.managed_by == ManagedBy::Unmanaged)
                && server.managed_by == ManagedBy::CommonKit
            {
                return Err(RelayLifecycleError::OwnershipConflict(server.id));
            }
            let tools = if server.enabled {
                validator.discover(&server)?
            } else {
                Vec::new()
            };
            prepared.push(RelayRuntimeServer {
                config: server,
                tools,
            });
        }

        let mut candidate = self.servers.clone();
        let mut summary = ReconcileSummary {
            added: Vec::new(),
            updated: Vec::new(),
            removed: Vec::new(),
        };
        for server in prepared {
            if candidate.contains_key(&server.config.id) {
                summary.updated.push(server.config.id.clone());
            } else {
                summary.added.push(server.config.id.clone());
            }
            candidate.insert(server.config.id.clone(), server);
        }
        if prune_commonkit {
            candidate.retain(|id, server| {
                let remove =
                    server.config.managed_by == ManagedBy::CommonKit && !desired_ids.contains(id);
                if remove {
                    summary.removed.push(id.clone());
                }
                !remove
            });
        }
        let candidate_catalog = Self { servers: candidate };
        candidate_catalog.localized_tools()?;
        *self = candidate_catalog;
        Ok(summary)
    }
}

pub fn localize_tool(server_id: &str, tool: &RelayTool) -> RelayTool {
    let name = tool
        .name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    RelayTool {
        name: format!("{server_id}__{name}"),
        description: format!(
            "[{server_id}] {}",
            if tool.description.is_empty() {
                &tool.name
            } else {
                &tool.description
            }
        ),
        input_schema: tool.input_schema.clone(),
    }
}

impl RelayConfig {
    pub fn normalize(raw: Value) -> Result<Self, RelayConfigError> {
        let object = raw.as_object().ok_or(RelayConfigError::NotObject)?;
        let listen_value = object.get("listen").or_else(|| object.get("admin"));
        let listen = normalize_listen(listen_value)?;
        let servers = object
            .get("servers")
            .and_then(Value::as_array)
            .map_or(Ok(Vec::new()), |servers| {
                servers.iter().map(normalize_server).collect()
            })?;
        let mut ids = BTreeSet::new();
        if servers.iter().any(|server| !ids.insert(server.id.clone())) {
            return Err(RelayConfigError::DuplicateServerId);
        }
        Ok(Self { listen, servers })
    }
}

fn normalize_listen(value: Option<&Value>) -> Result<RelayListenConfig, RelayConfigError> {
    let value = value.and_then(Value::as_object);
    let host = value
        .and_then(|value| value.get("host"))
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1");
    if !matches!(host, "127.0.0.1" | "::1") {
        return Err(RelayConfigError::NonLoopbackListen);
    }
    let port = value
        .and_then(|value| value.get("port"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_PORT.into())
        .try_into()
        .map_err(|_| RelayConfigError::InvalidPort)?;
    let path = value
        .and_then(|value| value.get("path").or_else(|| value.get("mcpPath")))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_MCP_PATH);
    if !path.starts_with('/') || path.contains('?') || path.contains('#') {
        return Err(RelayConfigError::InvalidMcpPath);
    }
    Ok(RelayListenConfig {
        host: host.into(),
        port,
        path: path.into(),
    })
}

fn normalize_server(value: &Value) -> Result<RelayServerConfig, RelayConfigError> {
    let value = value.as_object().ok_or(RelayConfigError::ServerNotObject)?;
    let id = RelayServerId::parse(
        value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(RelayConfigError::InvalidServerId)?,
    )?;
    let remote = value
        .get("remote")
        .and_then(Value::as_object)
        .ok_or(RelayConfigError::MissingRemote)?;
    if remote.get("type").and_then(Value::as_str) != Some("streamable_http") {
        return Err(RelayConfigError::InvalidTransport);
    }
    let remote_url = remote
        .get("url")
        .and_then(Value::as_str)
        .ok_or(RelayConfigError::MissingRemoteUrl)?;
    validate_remote_url(remote_url)?;
    let headers = remote.get("headers").and_then(Value::as_object).map_or(
        Ok(BTreeMap::new()),
        |headers| {
            headers
                .iter()
                .map(|(name, value)| {
                    let value = value
                        .as_str()
                        .ok_or(RelayConfigError::LiteralHeaderSecret)?;
                    if !is_secret_reference(value) {
                        return Err(RelayConfigError::LiteralHeaderSecret);
                    }
                    Ok((name.clone(), value.into()))
                })
                .collect()
        },
    )?;
    let cache = value.get("cache").and_then(Value::as_object);
    let tools_ttl_ms = positive_ms(
        cache.and_then(|cache| cache.get("toolsTtlMs")),
        DEFAULT_TOOLS_TTL_MS,
    );
    let auto_refresh_ms = match cache.and_then(|cache| cache.get("autoRefreshMs")) {
        Some(Value::Bool(false)) => 0,
        Some(Value::Number(number)) if number.as_u64() == Some(0) => 0,
        input => positive_ms(input, tools_ttl_ms),
    };
    let mode = match value.get("mode").and_then(Value::as_str) {
        Some("posthog-cli") => RelayMode::PosthogCli,
        _ => RelayMode::GenericCached,
    };
    let menu = value.get("menu").map(normalize_menu).transpose()?;
    Ok(RelayServerConfig {
        id: id.clone(),
        name: string_or(value.get("name"), id.as_str()),
        description: string_or(value.get("description"), ""),
        category: string_or(value.get("category"), ""),
        enabled: value.get("enabled").and_then(Value::as_bool) != Some(false),
        mode,
        remote: RelayRemoteConfig {
            transport_type: "streamable_http".into(),
            url: remote_url.into(),
            headers,
        },
        env_file: value
            .get("envFile")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cache: RelayCacheConfig {
            tools_ttl_ms,
            auto_refresh_ms,
        },
        menu,
        managed_by: match value.get("managedBy").and_then(Value::as_str) {
            Some("unmanaged") => ManagedBy::Unmanaged,
            _ => ManagedBy::CommonKit,
        },
    })
}

fn normalize_menu(value: &Value) -> Result<RelayMenuConfig, RelayConfigError> {
    let value = value.as_object().ok_or(RelayConfigError::InvalidMenu)?;
    let status_url = value
        .get("statusUrl")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(url) = &status_url {
        validate_local_url(url, false)?;
    }
    let actions = value
        .get("actions")
        .and_then(Value::as_array)
        .map_or(Ok(Vec::new()), |actions| {
            actions.iter().map(normalize_menu_action).collect()
        })?;
    Ok(RelayMenuConfig {
        status_url,
        ttl_ms: positive_ms(value.get("ttlMs"), 15_000),
        actions,
    })
}

fn normalize_menu_action(value: &Value) -> Result<RelayMenuAction, RelayConfigError> {
    let value = value
        .as_object()
        .ok_or(RelayConfigError::InvalidMenuAction)?;
    let id = RelayServerId::parse(
        value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(RelayConfigError::InvalidMenuAction)?,
    )?;
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.is_empty())
        .ok_or(RelayConfigError::InvalidMenuAction)?;
    let tool = value.get("tool").and_then(Value::as_str).map(str::to_owned);
    let url = value.get("url").and_then(Value::as_str).map(str::to_owned);
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(url) = &url {
        validate_local_url(url, method.is_none())?;
    }
    if let Some(method) = &method
        && !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
    {
        return Err(RelayConfigError::InvalidMenuAction);
    }
    if method.is_some() && url.is_none() || tool.is_some() && url.is_some() {
        return Err(RelayConfigError::InvalidMenuAction);
    }
    let view = value.get("view").cloned();
    if tool.is_none() && url.is_none() && view.is_none() {
        return Err(RelayConfigError::InvalidMenuAction);
    }
    Ok(RelayMenuAction {
        id,
        label: label.into(),
        confirm: value.get("confirm").and_then(Value::as_bool) == Some(true),
        tool,
        args: value.get("args").cloned(),
        url,
        method,
        view,
        input: value.get("input").cloned(),
    })
}

fn validate_remote_url(raw: &str) -> Result<(), RelayConfigError> {
    let url = Url::parse(raw).map_err(|_| RelayConfigError::InvalidRemoteUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RelayConfigError::InvalidRemoteUrl);
    }
    Ok(())
}

fn validate_local_url(raw: &str, display: bool) -> Result<(), RelayConfigError> {
    let url = Url::parse(raw).map_err(|_| RelayConfigError::NonLocalMenuUrl)?;
    if display && url.scheme() == "file" {
        return Ok(());
    }
    if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "::1")) {
        return Err(RelayConfigError::NonLocalMenuUrl);
    }
    Ok(())
}

fn is_secret_reference(value: &str) -> bool {
    ["env:", "secret:", "bws:", "keychain:", "vault:"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn positive_ms(value: Option<&Value>, fallback: u64) -> u64 {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn string_or(value: Option<&Value>, fallback: &str) -> String {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .into()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelayConfigError {
    #[error("relay config must be an object")]
    NotObject,
    #[error("relay must listen on an explicit loopback address")]
    NonLoopbackListen,
    #[error("relay port is invalid")]
    InvalidPort,
    #[error("relay MCP path is invalid")]
    InvalidMcpPath,
    #[error("relay server config must be an object")]
    ServerNotObject,
    #[error("relay server ID is invalid")]
    InvalidServerId,
    #[error("relay server IDs must be unique")]
    DuplicateServerId,
    #[error("relay server remote config is required")]
    MissingRemote,
    #[error("only streamable_http upstreams are supported")]
    InvalidTransport,
    #[error("relay remote URL is required")]
    MissingRemoteUrl,
    #[error("relay remote URL is invalid")]
    InvalidRemoteUrl,
    #[error("literal relay header values are forbidden; use a secret reference")]
    LiteralHeaderSecret,
    #[error("relay menu config is invalid")]
    InvalidMenu,
    #[error("relay menu action is invalid")]
    InvalidMenuAction,
    #[error("relay menu URL must be loopback HTTP or a display-only file URL")]
    NonLocalMenuUrl,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelayLifecycleError {
    #[error("relay server appears more than once")]
    DuplicateServer,
    #[error("unmanaged relay server cannot be replaced without an ownership transfer: {0:?}")]
    OwnershipConflict(RelayServerId),
    #[error("upstream relay validation failed: {0}")]
    Validation(String),
    #[error("localized relay tool name collision: {0}")]
    ToolNameCollision(String),
}

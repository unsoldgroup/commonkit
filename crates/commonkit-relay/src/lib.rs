//! Rust MCP relay configuration and atomic desired-state lifecycle.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

mod runtime;
mod transaction;
mod upstream_http;
pub use runtime::{
    DownstreamRequest, PeerAddress, RelayErrorCode, RelayHealth, RelayRuntime, RuntimeNotification,
    UpstreamError, UpstreamManager,
};
pub use transaction::{
    LegacyRelayError, LegacyRelayReader, RelayAdapter, RelayLifecycleControl, RelayMutationInputs,
    RelayPlanError, RelayPlanRequest, plan_relay_operation,
};
pub use upstream_http::HttpUpstreamManager;

pub const DEFAULT_PORT: u16 = 3764;
pub const DEFAULT_MCP_PATH: &str = "/mcp";
pub const DEFAULT_TOOLS_TTL_MS: u64 = 15 * 60 * 1000;

static TEMP_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum RelayStorageError {
    #[error("relay storage root is invalid")]
    InvalidRoot,
    #[error("relay file escapes its approved root")]
    RootEscape,
    #[error("relay env file permissions are not private")]
    InsecureEnvPermissions,
    #[error("relay env files require a platform ACL verifier on Windows")]
    WindowsAclVerificationRequired,
    #[error("relay storage operation failed")]
    Io(#[source] std::io::Error),
    #[error("relay tool cache serialization failed")]
    Serialization(#[source] serde_json::Error),
}

impl From<std::io::Error> for RelayStorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RelayStorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[derive(Debug, Clone)]
pub struct EnvFileLoader {
    root: PathBuf,
}

impl EnvFileLoader {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, RelayStorageError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|_| RelayStorageError::InvalidRoot)?;
        if !root.is_dir() {
            return Err(RelayStorageError::InvalidRoot);
        }
        Ok(Self { root })
    }

    pub fn load(
        &self,
        path: Option<impl AsRef<Path>>,
    ) -> Result<BTreeMap<String, String>, RelayStorageError> {
        let Some(path) = path else {
            return Ok(BTreeMap::new());
        };
        let path = contained_path(&self.root, path.as_ref())?;
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let path = path.canonicalize()?;
        if !path.starts_with(&self.root) {
            return Err(RelayStorageError::RootEscape);
        }
        #[cfg(windows)]
        {
            Err(RelayStorageError::WindowsAclVerificationRequired)
        }
        #[cfg(not(windows))]
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(&path)?.permissions().mode() & 0o077 != 0 {
                    return Err(RelayStorageError::InsecureEnvPermissions);
                }
            }
            let content = fs::read_to_string(path)?;
            Ok(parse_env_file(&content))
        }
    }
}

fn parse_env_file(content: &str) -> BTreeMap<String, String> {
    content
        .lines()
        .filter_map(|raw_line| {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, raw_value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            let value = raw_value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCache {
    pub cached_at: u64,
    pub tools: Vec<RelayTool>,
}

#[derive(Debug, Clone)]
pub struct ToolCacheStore {
    root: PathBuf,
}

impl ToolCacheStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, RelayStorageError> {
        fs::create_dir_all(root.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.as_ref(), fs::Permissions::from_mode(0o700))?;
        }
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(RelayStorageError::InvalidRoot);
        }
        Ok(Self { root })
    }

    pub fn load(&self, id: &RelayServerId) -> Result<Option<ToolCache>, RelayStorageError> {
        let path = self.cache_path(id);
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_str(&content).ok())
    }

    pub fn load_or_retain(
        &self,
        id: &RelayServerId,
        cache: &mut ToolCache,
    ) -> Result<bool, RelayStorageError> {
        let Some(loaded) = self.load(id)? else {
            return Ok(false);
        };
        *cache = loaded;
        Ok(true)
    }

    pub fn save(&self, id: &RelayServerId, cache: &ToolCache) -> Result<(), RelayStorageError> {
        let destination = self.cache_path(id);
        let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
        let temporary = self.root.join(format!(
            ".{}.{}.{}.tmp",
            id.as_str(),
            std::process::id(),
            nonce
        ));
        let bytes = serde_json::to_vec_pretty(cache)?;
        let result = (|| -> Result<(), RelayStorageError> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, destination)?;
            sync_directory(&self.root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn cache_path(&self, id: &RelayServerId) -> PathBuf {
        self.root.join(format!("{}.tools.json", id.as_str()))
    }
}

fn contained_path(root: &Path, requested: &Path) -> Result<PathBuf, RelayStorageError> {
    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(RelayStorageError::RootEscape);
    }
    let path = if requested.is_absolute() {
        requested.to_owned()
    } else {
        root.join(requested)
    };
    if !path.starts_with(root) {
        return Err(RelayStorageError::RootEscape);
    }
    Ok(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), RelayStorageError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), RelayStorageError> {
    Ok(())
}

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

/// Narrow, provider-neutral representation of resolved portable MCP declarations.
///
/// APM integrations may populate this DTO from documented manifest and lockfile
/// fields. CommonKit deliberately does not import or expose provider internals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedMcpDeclarations {
    pub contract_version: String,
    pub declarations: Vec<PortableMcpDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableMcpDeclaration {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub resolution: PortableMcpResolution,
    pub provenance: McpDeclarationProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableMcpResolution {
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, String>,
    },
    /// APM 0.25.0 does not document enough resolved registry state to safely
    /// translate these entries without depending on Python internals.
    RegistryReference { reference: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpDeclarationProvenance {
    pub provider_id: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayClientConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub transport_type: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvergedRelayState {
    pub relay: RelayConfig,
    pub client: RelayClientConfig,
    pub provenance: BTreeMap<RelayServerId, McpDeclarationProvenance>,
}

impl ConvergedRelayState {
    /// Relay mutations always travel through CommonKit's plan-bound consent path.
    pub fn requires_confirmation(&self) -> bool {
        true
    }
}

pub fn converge_provider_mcp(
    resolved: ResolvedMcpDeclarations,
) -> Result<ConvergedRelayState, RelayConvergenceError> {
    if resolved.contract_version != "commonkit.resolved-mcp.v1" {
        return Err(RelayConvergenceError::UnsupportedContractVersion);
    }
    let listen = RelayListenConfig {
        host: "127.0.0.1".into(),
        port: DEFAULT_PORT,
        path: DEFAULT_MCP_PATH.into(),
    };
    let mut servers = Vec::with_capacity(resolved.declarations.len());
    let mut provenance = BTreeMap::new();
    for declaration in resolved.declarations {
        if declaration.provenance.provider_id.trim().is_empty()
            || declaration.provenance.source.trim().is_empty()
        {
            return Err(RelayConvergenceError::MissingProvenance);
        }
        let id = RelayServerId::parse(declaration.id)?;
        let (url, headers) = match declaration.resolution {
            PortableMcpResolution::StreamableHttp { url, headers } => (url, headers),
            PortableMcpResolution::RegistryReference { .. } => {
                return Err(RelayConvergenceError::ResolvedMcpInterfaceUnsupported);
            }
        };
        validate_remote_url(&url)?;
        if headers.values().any(|value| !is_secret_reference(value)) {
            return Err(RelayConvergenceError::Config(
                RelayConfigError::LiteralHeaderSecret,
            ));
        }
        if provenance
            .insert(id.clone(), declaration.provenance)
            .is_some()
        {
            return Err(RelayConvergenceError::Config(
                RelayConfigError::DuplicateServerId,
            ));
        }
        servers.push(RelayServerConfig {
            id,
            name: declaration.name,
            description: String::new(),
            category: String::new(),
            enabled: declaration.enabled,
            mode: RelayMode::GenericCached,
            remote: RelayRemoteConfig {
                transport_type: "streamable_http".into(),
                url,
                headers,
            },
            env_file: None,
            cache: RelayCacheConfig {
                tools_ttl_ms: DEFAULT_TOOLS_TTL_MS,
                auto_refresh_ms: DEFAULT_TOOLS_TTL_MS,
            },
            menu: None,
            managed_by: ManagedBy::CommonKit,
        });
    }
    servers.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(ConvergedRelayState {
        relay: RelayConfig { listen, servers },
        client: RelayClientConfig {
            name: "commonkit-relay".into(),
            transport_type: "streamable_http".into(),
            url: format!("http://127.0.0.1:{DEFAULT_PORT}{DEFAULT_MCP_PATH}"),
        },
        provenance,
    })
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
    #[serde(default)]
    pub description: String,
    #[serde(default)]
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
pub enum RelayConvergenceError {
    #[error("resolved MCP declaration contract version is unsupported")]
    UnsupportedContractVersion,
    #[error("resolved MCP declaration provenance is required")]
    MissingProvenance,
    #[error(
        "resolved_mcp_interface_unsupported: registry MCP entries require a documented provider export"
    )]
    ResolvedMcpInterfaceUnsupported,
    #[error(transparent)]
    Config(#[from] RelayConfigError),
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

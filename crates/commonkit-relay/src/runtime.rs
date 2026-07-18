use std::collections::BTreeMap;

use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    RelayCatalog, RelayLifecycleError, RelayRuntimeServer, RelayServerConfig, RelayServerId,
    RelayTool, UpstreamValidator,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAddress {
    Loopback,
    Remote,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownstreamRequest {
    ListTools,
    CallTool { name: String, arguments: Value },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayHealth {
    Healthy,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeNotification {
    ToolsListChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayErrorCode {
    Unauthorized,
    Forbidden,
    ToolNotFound,
    UpstreamUnavailable,
    InvalidState,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UpstreamError {
    #[error("upstream unavailable")]
    Unavailable(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct RuntimeError {
    code: RelayErrorCode,
    message: &'static str,
}

impl RuntimeError {
    pub fn code(&self) -> RelayErrorCode {
        self.code
    }

    fn new(code: RelayErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }
}

/// Owns persistent connections or processes for configured upstreams.
///
/// Implementations must keep credential material inside the transport boundary;
/// errors crossing this trait are deliberately reduced to a redacted category.
pub trait UpstreamManager {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, UpstreamError>;
    fn call(
        &self,
        server: &RelayServerConfig,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, UpstreamError>;
    fn health(&self, server: &RelayServerConfig) -> RelayHealth;
    fn reconnect(&self, server: &RelayServerConfig) -> Result<(), UpstreamError>;
}

struct ManagerValidator<'a, M>(&'a M);

impl<M: UpstreamManager> UpstreamValidator for ManagerValidator<'_, M> {
    fn discover(&self, server: &RelayServerConfig) -> Result<Vec<RelayTool>, RelayLifecycleError> {
        self.0
            .discover(server)
            .map_err(|_| RelayLifecycleError::Validation("upstream unavailable".into()))
    }
}

/// In-process relay state behind the loopback Streamable HTTP listener.
///
/// The listener maps authenticated JSON-RPC methods onto `handle`; keeping the
/// state machine independent makes transport parsing unable to bypass policy.
pub struct RelayRuntime<M> {
    bearer_token: String,
    upstreams: M,
    catalog: RelayCatalog,
    refreshed_at: BTreeMap<RelayServerId, u64>,
    health: BTreeMap<RelayServerId, RelayHealth>,
    notifications: Vec<RuntimeNotification>,
}

impl<M: UpstreamManager> RelayRuntime<M> {
    pub fn new(
        bearer_token: impl Into<String>,
        upstreams: M,
        desired: Vec<RelayServerConfig>,
        now_ms: u64,
    ) -> Result<Self, RelayLifecycleError> {
        let bearer_token = bearer_token.into();
        let mut catalog = RelayCatalog::default();
        catalog.reconcile(desired, true, &ManagerValidator(&upstreams))?;
        let refreshed_at = catalog
            .servers()
            .map(|server| (server.config.id.clone(), now_ms))
            .collect();
        let health = catalog
            .servers()
            .map(|server| (server.config.id.clone(), RelayHealth::Healthy))
            .collect();
        Ok(Self {
            bearer_token,
            upstreams,
            catalog,
            refreshed_at,
            health,
            notifications: vec![RuntimeNotification::ToolsListChanged],
        })
    }

    pub fn handle(
        &self,
        peer: PeerAddress,
        authorization: Option<&str>,
        request: DownstreamRequest,
    ) -> Result<Value, RuntimeError> {
        if peer != PeerAddress::Loopback {
            return Err(RuntimeError::new(
                RelayErrorCode::Forbidden,
                "relay is loopback only",
            ));
        }
        let expected = format!("Bearer {}", self.bearer_token);
        if authorization != Some(expected.as_str()) {
            return Err(RuntimeError::new(
                RelayErrorCode::Unauthorized,
                "relay authentication failed",
            ));
        }
        match request {
            DownstreamRequest::ListTools => Ok(json!({ "tools": self.list_tools()? })),
            DownstreamRequest::CallTool { name, arguments } => {
                let (server, upstream_name) = self.resolve_tool(&name)?;
                self.upstreams
                    .call(&server.config, upstream_name, arguments)
                    .map_err(|_| {
                        RuntimeError::new(
                            RelayErrorCode::UpstreamUnavailable,
                            "upstream relay unavailable",
                        )
                    })
            }
        }
    }

    pub fn list_tools(&self) -> Result<Vec<RelayTool>, RuntimeError> {
        self.catalog.localized_tools().map_err(|_| {
            RuntimeError::new(RelayErrorCode::InvalidState, "relay catalog is invalid")
        })
    }

    pub fn reconcile(
        &mut self,
        desired: Vec<RelayServerConfig>,
        prune_commonkit: bool,
        now_ms: u64,
    ) -> Result<(), RelayLifecycleError> {
        let before = self.catalog.localized_tools()?;
        self.catalog
            .reconcile(desired, prune_commonkit, &ManagerValidator(&self.upstreams))?;
        self.refreshed_at = self
            .catalog
            .servers()
            .map(|server| (server.config.id.clone(), now_ms))
            .collect();
        self.health = self
            .catalog
            .servers()
            .map(|server| (server.config.id.clone(), RelayHealth::Healthy))
            .collect();
        if self.catalog.localized_tools()? != before {
            self.notify_tools_changed();
        }
        Ok(())
    }

    /// Executes one deterministic background-refresh cycle. Discovery failures
    /// retain the last known tool set; reconnect is attempted independently.
    pub fn refresh_due(&mut self, now_ms: u64) {
        let due = self
            .catalog
            .servers()
            .filter(|server| {
                let interval = server.config.cache.auto_refresh_ms;
                interval > 0
                    && now_ms.saturating_sub(
                        self.refreshed_at
                            .get(&server.config.id)
                            .copied()
                            .unwrap_or(0),
                    ) >= interval
            })
            .map(|server| server.config.clone())
            .collect::<Vec<_>>();
        for config in due {
            self.refreshed_at.insert(config.id.clone(), now_ms);
            if self.upstreams.health(&config) == RelayHealth::Unavailable {
                let _ = self.upstreams.reconnect(&config);
            }
            let current = self
                .catalog
                .servers()
                .find(|server| server.config.id == config.id)
                .cloned();
            let Some(current) = current else { continue };
            match self.upstreams.discover(&config) {
                Ok(tools) => {
                    self.health.insert(config.id.clone(), RelayHealth::Healthy);
                    if tools != current.tools {
                        let mut servers = self.catalog.servers().cloned().collect::<Vec<_>>();
                        if let Some(server) = servers
                            .iter_mut()
                            .find(|server| server.config.id == config.id)
                        {
                            server.tools = tools;
                        }
                        if let Ok(candidate) = RelayCatalog::from_servers(servers)
                            && candidate.localized_tools().is_ok()
                        {
                            self.catalog = candidate;
                            self.notify_tools_changed();
                        }
                    }
                }
                Err(_) => {
                    // A successful reconnect establishes transport health even
                    // when discovery itself remains temporarily unavailable.
                    self.health
                        .insert(config.id.clone(), self.upstreams.health(&config));
                }
            }
        }
    }

    pub fn health(&self, id: &RelayServerId) -> Option<RelayHealth> {
        self.health.get(id).copied()
    }

    pub fn take_notifications(&mut self) -> Vec<RuntimeNotification> {
        std::mem::take(&mut self.notifications)
    }

    fn notify_tools_changed(&mut self) {
        if !self
            .notifications
            .contains(&RuntimeNotification::ToolsListChanged)
        {
            self.notifications
                .push(RuntimeNotification::ToolsListChanged);
        }
    }

    fn resolve_tool(
        &self,
        localized_name: &str,
    ) -> Result<(&RelayRuntimeServer, &str), RuntimeError> {
        for server in self
            .catalog
            .servers()
            .filter(|server| server.config.enabled)
        {
            for tool in &server.tools {
                if crate::localize_tool(server.config.id.as_str(), tool).name == localized_name {
                    return Ok((server, tool.name.as_str()));
                }
            }
        }
        Err(RuntimeError::new(
            RelayErrorCode::ToolNotFound,
            "relay tool not found",
        ))
    }
}

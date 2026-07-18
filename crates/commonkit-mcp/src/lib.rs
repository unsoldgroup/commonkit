//! Agent-facing MCP tools backed by the authenticated CommonKit daemon.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use commonkit_service::{ControlToken, DaemonDiscovery};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

pub type BackendFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, BackendError>> + Send + 'a>>;

pub trait ControlBackend: Send + Sync + 'static {
    fn get(&self, path: &'static str) -> BackendFuture<'_>;
    fn apply(&self, input: ApplyPlanInput) -> BackendFuture<'_>;
}

#[derive(Clone)]
pub struct DaemonBackend {
    discovery_path: PathBuf,
    token_path: PathBuf,
    client: reqwest::Client,
}

impl DaemonBackend {
    pub fn discover() -> Result<Self, BackendError> {
        let paths = commonkit_platform::AppPaths::discover()?;
        Ok(Self {
            discovery_path: paths.state.join("daemon.json"),
            token_path: paths.config.join("control.token"),
            client: reqwest::Client::builder().build()?,
        })
    }

    async fn connection(&self) -> Result<(String, HeaderValue), BackendError> {
        let discovery: DaemonDiscovery =
            serde_json::from_slice(&tokio::fs::read(&self.discovery_path).await?)?;
        let token = ControlToken::load(&self.token_path)?;
        let authorization = HeaderValue::from_str(&format!("Bearer {}", token.expose_for_client()))
            .map_err(|_| BackendError::InvalidAuthorization)?;
        Ok((
            format!("http://127.0.0.1:{}", discovery.port),
            authorization,
        ))
    }
}

impl ControlBackend for DaemonBackend {
    fn get(&self, path: &'static str) -> BackendFuture<'_> {
        Box::pin(async move {
            let (base, authorization) = self.connection().await?;
            let response = self
                .client
                .get(format!("{base}{path}"))
                .header(AUTHORIZATION, authorization)
                .send()
                .await?;
            decode(response).await
        })
    }

    fn apply(&self, input: ApplyPlanInput) -> BackendFuture<'_> {
        Box::pin(async move {
            let (base, authorization) = self.connection().await?;
            let response = self
                .client
                .post(format!("{base}/control/v1/plans/{}/apply", input.plan_id))
                .header(AUTHORIZATION, authorization)
                .header("idempotency-key", &input.idempotency_key)
                .json(&json!({
                    "confirmed": input.confirmed,
                    "confirmationId": input.confirmation_id,
                }))
                .send()
                .await?;
            decode(response).await
        })
    }
}

async fn decode(response: reqwest::Response) -> Result<Value, BackendError> {
    let status = response.status();
    let value: Value = response.json().await?;
    if status.is_success() {
        Ok(value)
    } else {
        Err(BackendError::Daemon {
            status: status.as_u16(),
            code: value
                .pointer("/error/code")
                .and_then(Value::as_str)
                .unwrap_or("daemon_error")
                .into(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyPlanInput {
    pub plan_id: String,
    pub confirmed: bool,
    pub confirmation_id: String,
    pub idempotency_key: String,
}

#[derive(Clone)]
pub struct CommonKitMcp {
    backend: Arc<dyn ControlBackend>,
}

impl CommonKitMcp {
    pub fn new(backend: Arc<dyn ControlBackend>) -> Self {
        Self { backend }
    }

    pub fn published_tool_names() -> Vec<String> {
        Self::tool_router()
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }
}

#[tool_router]
impl CommonKitMcp {
    #[tool(
        name = "commonkit_get_status",
        description = "Read CommonKit daemon, target, loadout, and drift status. This tool never mutates state."
    )]
    pub async fn get_status(&self) -> Result<CallToolResult, ErrorData> {
        tool_result(self.backend.get("/control/v1/status").await)
    }

    #[tool(
        name = "commonkit_export_diagnostics",
        description = "Export schema-bound redacted CommonKit diagnostics. Secret values are never included."
    )]
    pub async fn export_diagnostics(&self) -> Result<CallToolResult, ErrorData> {
        tool_result(self.backend.get("/control/v1/diagnostics").await)
    }

    #[tool(
        name = "commonkit_apply_plan",
        description = "Apply a previously registered content-addressed plan. Requires explicit user confirmation and an idempotency key."
    )]
    pub async fn apply_plan(
        &self,
        Parameters(input): Parameters<ApplyPlanInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if !input.confirmed {
            return Ok(CallToolResult::structured_error(json!({
                "code": "confirmation_required",
                "message": "Explicit user confirmation is required",
                "retryable": false,
            })));
        }
        tool_result(self.backend.apply(input).await)
    }
}

fn tool_result(result: Result<Value, BackendError>) -> Result<CallToolResult, ErrorData> {
    Ok(match result {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => CallToolResult::structured_error(json!({
            "code": error.code(),
            "message": error.safe_message(),
            "retryable": error.retryable(),
        })),
    })
}

#[tool_handler(
    name = "commonkit",
    instructions = "Inspect status and plans before mutation. Apply requires explicit user confirmation."
)]
impl ServerHandler for CommonKitMcp {}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("CommonKit platform paths are unavailable")]
    Platform(#[from] commonkit_platform::PlatformError),
    #[error("CommonKit daemon discovery is unavailable")]
    Io(#[from] std::io::Error),
    #[error("CommonKit daemon discovery is invalid")]
    Json(#[from] serde_json::Error),
    #[error("CommonKit daemon request failed")]
    Http(#[from] reqwest::Error),
    #[error("CommonKit control token is unavailable")]
    Service(#[from] commonkit_service::ServiceError),
    #[error("CommonKit authorization header is invalid")]
    InvalidAuthorization,
    #[error("CommonKit daemon returned {status}: {code}")]
    Daemon { status: u16, code: String },
}

impl BackendError {
    fn code(&self) -> &str {
        match self {
            Self::Platform(_) => "platform_unavailable",
            Self::Io(_) => "daemon_offline",
            Self::Json(_) => "discovery_invalid",
            Self::Http(_) => "daemon_unreachable",
            Self::Service(_) | Self::InvalidAuthorization => "authorization_unavailable",
            Self::Daemon { code, .. } => code,
        }
    }

    fn safe_message(&self) -> &'static str {
        match self {
            Self::Daemon { .. } => "The CommonKit daemon rejected the request",
            _ => "The CommonKit daemon is unavailable",
        }
    }

    fn retryable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Http(_))
    }
}

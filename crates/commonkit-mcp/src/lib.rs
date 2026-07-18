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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub type BackendFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, BackendError>> + Send + 'a>>;

pub trait ControlBackend: Send + Sync + 'static {
    fn get(&self, path: &'static str) -> BackendFuture<'_>;
    fn apply(&self, input: ApplyPlanInput) -> BackendFuture<'_>;
    fn post(&self, path: &'static str, input: Value) -> BackendFuture<'_> {
        let _ = (path, input);
        Box::pin(async { Err(BackendError::Unsupported) })
    }
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

    fn post(&self, path: &'static str, input: Value) -> BackendFuture<'_> {
        Box::pin(async move {
            let (base, authorization) = self.connection().await?;
            let response = self
                .client
                .post(format!("{base}{path}"))
                .header(AUTHORIZATION, authorization)
                .json(&input)
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyPlanInput {
    pub plan_id: String,
    pub confirmed: bool,
    pub confirmation_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadInput {
    pub target_id: Option<String>,
    pub pointer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsentInput {
    pub confirmed: bool,
    pub confirmation_id: String,
    pub idempotency_key: String,
}

impl ConsentInput {
    pub fn denied() -> Self {
        Self {
            confirmed: false,
            confirmation_id: "not-confirmed".into(),
            idempotency_key: "not-confirmed".into(),
        }
    }
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
        name = "commonkit_compose",
        description = "Compute the composed desired state and provenance without mutating a target."
    )]
    pub async fn compose(
        &self,
        Parameters(input): Parameters<ReadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = read_input_error(&input) {
            return Ok(error);
        }
        tool_result(self.backend.get("/control/v1/compose").await)
    }

    #[tool(
        name = "commonkit_explain",
        description = "Explain desired-state and policy provenance without mutating a target."
    )]
    pub async fn explain(
        &self,
        Parameters(input): Parameters<ReadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = read_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/explain",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
    }

    #[tool(
        name = "commonkit_verify",
        description = "Verify managed target parity and provider integrity without mutation."
    )]
    pub async fn verify(
        &self,
        Parameters(input): Parameters<ReadInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = read_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/verify",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
    }

    #[tool(
        name = "commonkit_plan_sync",
        description = "Plan a target synchronization. Requires explicit user confirmation because provider staging may access configured sources."
    )]
    pub async fn plan_sync(
        &self,
        Parameters(input): Parameters<ConsentInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = consent_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/sync/plan",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
    }

    #[tool(
        name = "commonkit_snapshot_create",
        description = "Create a managed mutable-state snapshot. Requires explicit user confirmation."
    )]
    pub async fn snapshot_create(
        &self,
        Parameters(input): Parameters<ConsentInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = consent_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/snapshots",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
    }

    #[tool(
        name = "commonkit_snapshot_restore",
        description = "Restore a managed mutable-state snapshot. Requires explicit user confirmation."
    )]
    pub async fn snapshot_restore(
        &self,
        Parameters(input): Parameters<ConsentInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = consent_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/snapshots/restore",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
    }

    #[tool(
        name = "commonkit_rollback",
        description = "Rollback a CommonKit run from its durable receipt. Requires explicit user confirmation."
    )]
    pub async fn rollback(
        &self,
        Parameters(input): Parameters<ConsentInput>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(error) = consent_input_error(&input) {
            return Ok(error);
        }
        tool_result(
            self.backend
                .post(
                    "/control/v1/rollback",
                    serde_json::to_value(&input).unwrap_or(Value::Null),
                )
                .await,
        )
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
        Ok(value) if serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() <= 256 * 1024) => {
            CallToolResult::structured(value)
        }
        Ok(_) => input_error(
            "response_too_large",
            "The daemon response exceeded the safe output limit",
        ),
        Err(error) => CallToolResult::structured_error(json!({
            "code": error.code(),
            "message": error.safe_message(),
            "retryable": error.retryable(),
        })),
    })
}

fn read_input_error(input: &ReadInput) -> Option<CallToolResult> {
    if input
        .target_id
        .as_ref()
        .is_some_and(|value| value.len() > 128)
        || input
            .pointer
            .as_ref()
            .is_some_and(|value| value.len() > 1024)
    {
        return Some(input_error(
            "invalid_input",
            "Input exceeds the supported size",
        ));
    }
    None
}

fn consent_input_error(input: &ConsentInput) -> Option<CallToolResult> {
    if !input.confirmed {
        return Some(input_error(
            "confirmation_required",
            "Explicit user confirmation is required",
        ));
    }
    if input.confirmation_id.is_empty()
        || input.confirmation_id.len() > 128
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > 128
    {
        return Some(input_error(
            "invalid_input",
            "Confirmation fields are invalid",
        ));
    }
    None
}

fn input_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(
        json!({ "code": code, "message": message, "retryable": false }),
    )
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
    #[error("the requested CommonKit capability is not implemented by this backend")]
    Unsupported,
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
            Self::Unsupported => "capability_unavailable",
        }
    }

    fn safe_message(&self) -> &'static str {
        match self {
            Self::Daemon { .. } => "The CommonKit daemon rejected the request",
            Self::Unsupported => {
                "This CommonKit capability is not available in the configured backend"
            }
            _ => "The CommonKit daemon is unavailable",
        }
    }

    fn retryable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Http(_))
    }
}

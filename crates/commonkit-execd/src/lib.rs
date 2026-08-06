//! Authenticated network API for durable remote execution.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use commonkit_contracts::{
    ExecutionManifest, ExecutionTarget, JobState, Lease, NetworkPolicy, StableId,
};
use commonkit_execution::{ExecutionError, LocalObjectStore, Scheduler, SubmitOutcome};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    Submit,
    Read,
    Cancel,
    Worker,
}
#[derive(Clone)]
struct Identity {
    name: String,
    capabilities: BTreeSet<Capability>,
}

#[derive(Clone)]
pub struct ApiState {
    pub(crate) scheduler: Arc<Mutex<Scheduler>>,
    identities: Arc<BTreeMap<String, Identity>>,
    require_tls: bool,
    objects: Option<LocalObjectStore>,
    artifact_signing_key: Option<Arc<String>>,
    policy: Option<Arc<ExecutionPolicy>>,
    webhook: Option<Arc<webhook::WebhookConfig>>,
}

pub mod github;

pub mod webhook;

pub mod secrets;

pub mod workspace;

pub mod worker;

impl ApiState {
    pub fn audit_entries(&self) -> Result<Vec<commonkit_execution::AuditEntry>, ExecutionError> {
        self.scheduler.lock().unwrap().audit_entries()
    }
}

impl ApiState {
    pub fn new(
        scheduler: Scheduler,
        tokens: Vec<(String, String, BTreeSet<Capability>)>,
        require_tls: bool,
    ) -> Self {
        let identities = tokens
            .into_iter()
            .map(|(token, name, capabilities)| {
                (token_hash(&token), Identity { name, capabilities })
            })
            .collect();
        Self {
            scheduler: Arc::new(Mutex::new(scheduler)),
            identities: Arc::new(identities),
            require_tls,
            objects: None,
            artifact_signing_key: None,
            policy: None,
            webhook: None,
        }
    }

    pub fn with_object_store(
        mut self,
        objects: LocalObjectStore,
        signing_key: impl Into<String>,
    ) -> Self {
        self.objects = Some(objects);
        self.artifact_signing_key = Some(Arc::new(signing_key.into()));
        self
    }
    pub fn with_policy(mut self, policy: ExecutionPolicy) -> Self {
        self.policy = Some(Arc::new(policy));
        self
    }
    pub fn with_github_webhook(mut self, config: webhook::WebhookConfig) -> Self {
        self.webhook = Some(Arc::new(config));
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionPolicy {
    pub allowed_repositories: BTreeSet<String>,
    pub max_cpu_millis: u32,
    pub max_memory_mib: u64,
    pub max_disk_mib: u64,
    pub allow_network: bool,
    pub allow_repository_write: bool,
}
impl ExecutionPolicy {
    fn validate(&self, manifest: &ExecutionManifest) -> Result<(), ApiError> {
        if !self.allowed_repositories.contains(&manifest.repository) {
            return Err(ApiError::forbidden("repository_denied"));
        }
        if manifest.resources.cpu_millis > self.max_cpu_millis
            || manifest.resources.memory_mib > self.max_memory_mib
            || manifest.resources.disk_mib > self.max_disk_mib
        {
            return Err(ApiError::forbidden("resource_policy_denied"));
        }
        if !self.allow_network && manifest.network_policy != NetworkPolicy::Deny {
            return Err(ApiError::forbidden("network_policy_denied"));
        }
        if !self.allow_repository_write && manifest.repository_write {
            return Err(ApiError::forbidden("repository_write_denied"));
        }
        Ok(())
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/execution/v1/jobs", post(submit))
        .route("/execution/v1/jobs/{job_id}", get(status))
        .route("/execution/v1/jobs/{job_id}/events", get(events))
        .route("/execution/v1/jobs/{job_id}/cancel", post(cancel))
        .route("/execution/v1/jobs/{job_id}/retry", post(retry))
        .route("/execution/v1/jobs/{job_id}/resume", post(resume))
        .route("/execution/v1/jobs/{job_id}/logs", get(logs))
        .route("/execution/v1/jobs/{job_id}/artifacts", get(artifacts))
        .route(
            "/execution/v1/artifacts/{artifact_id}/access",
            post(artifact_access),
        )
        .route(
            "/execution/v1/artifacts/{artifact_id}/content",
            get(artifact_content),
        )
        .route("/execution/v1/worker/lease", post(acquire))
        .route("/execution/v1/worker/renew", post(renew))
        .route("/execution/v1/worker/checkpoint", post(checkpoint))
        .route("/execution/v1/worker/complete", post(complete))
        .route("/execution/v1/worker/heartbeat", post(heartbeat))
        .route("/execution/v1/targets", get(targets))
        .route("/execution/v1/github/webhook", post(webhook::receive))
        .with_state(state)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitRequest {
    manifest: ExecutionManifest,
    idempotency_key: String,
}
async fn submit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<SubmitRequest>,
) -> ApiResult {
    let identity = authorize(&state, &headers, Capability::Submit)?;
    if let Some(policy) = &state.policy {
        if let Err(error) = policy.validate(&input.manifest) {
            let _ = state.scheduler.lock().unwrap().audit(
                now_ms(),
                &identity.name,
                "submit",
                "execution-api",
                false,
                error.code,
            );
            return Err(error);
        }
    }
    let now = now_ms();
    let outcome =
        state
            .scheduler
            .lock()
            .unwrap()
            .submit(&input.manifest, &input.idempotency_key, now)?;
    let (job_id, created) = match outcome {
        SubmitOutcome::Created(id) => (id, true),
        SubmitOutcome::Existing(id) => (id, false),
    };
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(json!({"jobId":job_id,"created":created})),
    ))
}
async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().snapshot(&job_id)?)?;
    Ok((StatusCode::OK, Json(value)))
}
#[derive(Deserialize)]
struct EventQuery {
    after: Option<u64>,
    limit: Option<u32>,
}
async fn events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().events(
        &job_id,
        query.after.unwrap_or(0),
        query.limit.unwrap_or(100),
    )?)?;
    Ok((StatusCode::OK, Json(value)))
}
async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Cancel)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().cancel(&job_id, now_ms())?)?;
    Ok((StatusCode::OK, Json(value)))
}
async fn retry(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Submit)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(
            state.scheduler.lock().unwrap().retry(&job_id, now_ms())?,
        )?),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResumeRequest {
    checkpoint_stage: String,
}
async fn resume(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(input): Json<ResumeRequest>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Submit)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(
            state
                .scheduler
                .lock()
                .unwrap()
                .resume(&job_id, &input.checkpoint_stage, now_ms())?,
        )?),
    ))
}
async fn logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    let logs: Vec<_> = state
        .scheduler
        .lock()
        .unwrap()
        .artifacts(&job_id)?
        .into_iter()
        .filter(|artifact| artifact.name.starts_with("diagnostics/"))
        .collect();
    Ok((StatusCode::OK, Json(serde_json::to_value(logs)?)))
}
async fn targets(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::to_value(
            state.scheduler.lock().unwrap().targets(now_ms(), 30_000)?,
        )?),
    ))
}

async fn artifacts(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::to_value(
            state.scheduler.lock().unwrap().artifacts(&job_id)?,
        )?),
    ))
}

async fn artifact_access(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(artifact_id): Path<String>,
) -> ApiResult {
    authorize(&state, &headers, Capability::Read)?;
    let _ = state.scheduler.lock().unwrap().artifact(&artifact_id)?;
    let expires = now_ms().saturating_add(300_000);
    let signature = artifact_signature(&state, &artifact_id, expires)?;
    Ok((
        StatusCode::OK,
        Json(
            json!({"url": format!("/execution/v1/artifacts/{artifact_id}/content?expires={expires}&signature={signature}"), "expiresAtUnixMs": expires}),
        ),
    ))
}

#[derive(Deserialize)]
struct ArtifactQuery {
    expires: u64,
    signature: String,
}
async fn artifact_content(
    State(state): State<ApiState>,
    Path(artifact_id): Path<String>,
    Query(query): Query<ArtifactQuery>,
) -> Result<axum::response::Response, ApiError> {
    if query.expires < now_ms()
        || artifact_signature(&state, &artifact_id, query.expires)? != query.signature
    {
        return Err(ApiError::forbidden("artifact_access_expired_or_invalid"));
    }
    let artifact = state.scheduler.lock().unwrap().artifact(&artifact_id)?;
    let store = state.objects.as_ref().ok_or(ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "artifact_store_unavailable",
    })?;
    let bytes = store.get(&artifact.object_digest)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, artifact.media_type)],
        bytes,
    )
        .into_response())
}

fn artifact_signature(
    state: &ApiState,
    artifact_id: &str,
    expires: u64,
) -> Result<String, ApiError> {
    let key = state.artifact_signing_key.as_ref().ok_or(ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "artifact_store_unavailable",
    })?;
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{key}\0{artifact_id}\0{expires}").as_bytes())
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcquireRequest {
    worker_id: StableId,
    target: ExecutionTarget,
    ttl_ms: u64,
}
async fn acquire(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<AcquireRequest>,
) -> ApiResult {
    let identity = authorize(&state, &headers, Capability::Worker)?;
    if identity.name != input.worker_id.as_str() {
        return Err(ApiError::forbidden("worker_identity_mismatch"));
    }
    let value = serde_json::to_value(state.scheduler.lock().unwrap().acquire(
        input.worker_id,
        &input.target,
        now_ms(),
        input.ttl_ms,
    )?)?;
    Ok((StatusCode::OK, Json(value)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenewRequest {
    lease: Lease,
    ttl_ms: u64,
}
async fn renew(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<RenewRequest>,
) -> ApiResult {
    authorize_worker(&state, &headers, &input.lease)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().renew(
        &input.lease,
        now_ms(),
        input.ttl_ms,
    )?)?;
    Ok((StatusCode::OK, Json(value)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckpointRequest {
    lease: Lease,
    stage: String,
    object_digest: commonkit_contracts::Sha256Digest,
}
async fn checkpoint(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<CheckpointRequest>,
) -> ApiResult {
    authorize_worker(&state, &headers, &input.lease)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().checkpoint(
        &input.lease,
        &input.stage,
        input.object_digest,
        now_ms(),
    )?)?;
    Ok((StatusCode::OK, Json(value)))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompleteRequest {
    lease: Lease,
    state: JobState,
    exit_code: Option<i32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeartbeatRequest {
    worker_id: StableId,
    target: ExecutionTarget,
}
async fn heartbeat(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<HeartbeatRequest>,
) -> ApiResult {
    let identity = authorize(&state, &headers, Capability::Worker)?;
    if identity.name != input.worker_id.as_str() {
        return Err(ApiError::forbidden("worker_identity_mismatch"));
    }
    state
        .scheduler
        .lock()
        .unwrap()
        .register_target(&input.worker_id, &input.target, now_ms())?;
    Ok((StatusCode::OK, Json(json!({"registered":true}))))
}
async fn complete(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<CompleteRequest>,
) -> ApiResult {
    authorize_worker(&state, &headers, &input.lease)?;
    let value = serde_json::to_value(state.scheduler.lock().unwrap().complete(
        &input.lease,
        input.state,
        input.exit_code,
        now_ms(),
    )?)?;
    Ok((StatusCode::OK, Json(value)))
}

fn authorize_worker<'a>(
    state: &'a ApiState,
    headers: &HeaderMap,
    lease: &Lease,
) -> Result<&'a Identity, ApiError> {
    let identity = authorize(state, headers, Capability::Worker)?;
    if identity.name != lease.worker_id.as_str() {
        Err(ApiError::forbidden("worker_identity_mismatch"))
    } else {
        Ok(identity)
    }
}
fn authorize<'a>(
    state: &'a ApiState,
    headers: &HeaderMap,
    capability: Capability,
) -> Result<&'a Identity, ApiError> {
    if state.require_tls
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            != Some("https")
    {
        audit_denial(state, "anonymous", capability, "tls_required");
        return Err(ApiError::forbidden("tls_required"));
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            audit_denial(state, "anonymous", capability, "authentication_required");
            ApiError::unauthorized("authentication_required")
        })?;
    let identity = state.identities.get(&token_hash(token)).ok_or_else(|| {
        audit_denial(state, "unknown", capability, "invalid_identity");
        ApiError::unauthorized("invalid_identity")
    })?;
    if !identity.capabilities.contains(&capability) {
        audit_denial(state, &identity.name, capability, "capability_denied");
        return Err(ApiError::forbidden("capability_denied"));
    }
    Ok(identity)
}
fn audit_denial(state: &ApiState, identity: &str, capability: Capability, reason: &str) {
    let _ = state.scheduler.lock().unwrap().audit(
        now_ms(),
        identity,
        &format!("{capability:?}").to_ascii_lowercase(),
        "execution-api",
        false,
        reason,
    );
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

type ApiResult = Result<(StatusCode, Json<Value>), ApiError>;
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
}
impl ApiError {
    fn unauthorized(code: &'static str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code,
        }
    }
    fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
        }
    }
}
impl From<ExecutionError> for ApiError {
    fn from(error: ExecutionError) -> Self {
        match error {
            ExecutionError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
            },
            ExecutionError::IdempotencyConflict => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
            },
            ExecutionError::StaleFence => Self {
                status: StatusCode::CONFLICT,
                code: "stale_fence",
            },
            ExecutionError::CursorExpired { .. } => Self {
                status: StatusCode::GONE,
                code: "cursor_expired",
            },
            _ => Self {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_request",
            },
        }
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(_: serde_json::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "serialization_failed",
        }
    }
}
impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(json!({"error":{"code":self.code,"message":"Request denied","retryable":false}})),
        )
            .into_response()
    }
}

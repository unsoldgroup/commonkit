//! Authenticated, loopback-only CommonKit local control API.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commonkit_contracts::{
    CONTRACT_VERSION, ComponentDiagnostic, DiagnosticBundle, DiagnosticState, Plan,
    RuntimeDiagnostic, SCHEMA_VERSION, SchemaVersion, Sha256Digest, StableId,
    assert_no_embedded_secrets, digest_domain_json,
};
use commonkit_core::{PlanDraft, build_plan};
use commonkit_reconcile::{PlanStore, PlanStoreError};
use futures_util::StreamExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

pub const API_VERSION: &str = "v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallState {
    Healthy,
    Drifted,
    Blocked,
    Applying,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceStatus {
    pub api_version: String,
    pub contract_version: String,
    pub schema_version: u32,
    pub runtime_version: String,
    pub state: OverallState,
    pub active_target: Option<String>,
    pub active_loadout: Option<String>,
    pub last_drift_check_unix_ms: Option<u128>,
    pub last_drift_error_code: Option<String>,
}

impl Default for ServiceStatus {
    fn default() -> Self {
        Self {
            api_version: API_VERSION.into(),
            contract_version: CONTRACT_VERSION.into(),
            schema_version: SCHEMA_VERSION,
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            state: OverallState::Offline,
            active_target: None,
            active_loadout: None,
            last_drift_check_unix_ms: None,
            last_drift_error_code: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftResult {
    pub state: OverallState,
    pub code: Option<String>,
}

pub trait DriftChecker: Send + Sync + 'static {
    fn check(&self) -> DriftResult;
}

pub struct DriftScheduler<C> {
    checker: Arc<C>,
    status: Arc<RwLock<ServiceStatus>>,
    events: EventHub,
}

impl<C: DriftChecker> DriftScheduler<C> {
    pub fn new(checker: Arc<C>, status: Arc<RwLock<ServiceStatus>>, events: EventHub) -> Self {
        Self {
            checker,
            status,
            events,
        }
    }

    pub async fn check_now(&self) -> DriftResult {
        let result = self.checker.check();
        let checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        {
            let mut status = self.status.write().await;
            status.state = result.state;
            status.last_drift_check_unix_ms = Some(checked_at);
            status.last_drift_error_code = result.code.clone();
        }
        self.events.publish(
            "drift.checked",
            serde_json::json!({
                "checkedAtUnixMs": checked_at,
                "state": result.state,
                "code": result.code,
            }),
        );
        result
    }

    pub async fn run_until<F>(&self, interval: Duration, shutdown: F)
    where
        F: std::future::Future<Output = ()>,
    {
        let mut timer = tokio::time::interval(interval.max(Duration::from_secs(1)));
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = timer.tick() => {
                    self.check_now().await;
                }
                _ = &mut shutdown => break,
            }
        }
    }
}

#[derive(Clone)]
pub struct ControlToken(Arc<[u8]>);

impl ControlToken {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(Arc::from(hex(&bytes).into_bytes()))
    }

    pub fn load_or_create(path: &Path) -> Result<Self, ServiceError> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => validate_token_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Ok(mut file) = std::fs::File::open(path) {
            return read_token(&mut file);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let token = Self::generate();
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(mut file) => {
                file.write_all(&token.0)?;
                file.sync_all()?;
                Ok(token)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                validate_token_file(path)?;
                read_token(&mut std::fs::File::open(path)?)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn load(path: &Path) -> Result<Self, ServiceError> {
        validate_token_file(path)?;
        read_token(&mut std::fs::File::open(path)?)
    }

    pub fn expose_for_client(&self) -> &str {
        std::str::from_utf8(&self.0).expect("generated token is ASCII")
    }

    fn matches(&self, candidate: &[u8]) -> bool {
        candidate.len() == self.0.len() && self.0.ct_eq(candidate).into()
    }
}

#[derive(Clone)]
struct ApiState {
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: String,
    events: EventHub,
    control: ControlPlane,
}

pub fn router(
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: impl Into<String>,
) -> Router {
    router_with_events(token, status, authority, EventHub::new(256))
}

pub fn router_with_events(
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: impl Into<String>,
    events: EventHub,
) -> Router {
    router_with_control(
        token,
        status,
        authority,
        events,
        ControlPlane::new(Arc::new(UnavailableExecutor)),
    )
}

pub fn router_with_control(
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: impl Into<String>,
    events: EventHub,
    control: ControlPlane,
) -> Router {
    let state = ApiState {
        token,
        status,
        authority: authority.into(),
        events,
        control,
    };
    Router::new()
        .route("/control/v1/status", get(get_status))
        .route("/control/v1/health", get(health))
        .route("/control/v1/events", get(get_events))
        .route("/control/v1/diagnostics", get(get_diagnostics))
        .route("/control/v1/plans", post(register_plan))
        .route("/control/v1/plans/{id}", get(get_plan))
        .route("/control/v1/plans/{id}/apply", post(apply_plan))
        .route("/control/v1/operations/{id}", get(get_operation))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlEvent {
    pub id: u64,
    pub event: String,
    pub data: Value,
}

#[derive(Clone)]
pub struct EventHub {
    inner: Arc<EventHubInner>,
}

struct EventHubInner {
    capacity: usize,
    history: std::sync::Mutex<VecDeque<ControlEvent>>,
    sender: tokio::sync::broadcast::Sender<ControlEvent>,
}

impl EventHub {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        Self {
            inner: Arc::new(EventHubInner {
                capacity,
                history: std::sync::Mutex::new(VecDeque::with_capacity(capacity)),
                sender,
            }),
        }
    }

    pub fn publish(&self, event: impl Into<String>, data: Value) -> ControlEvent {
        let mut history = self.inner.history.lock().expect("event history lock");
        let id = history.back().map_or(1, |event| event.id + 1);
        let event = ControlEvent {
            id,
            event: event.into(),
            data,
        };
        if history.len() == self.inner.capacity {
            history.pop_front();
        }
        history.push_back(event.clone());
        drop(history);
        let _ = self.inner.sender.send(event.clone());
        event
    }

    pub fn replay_after(&self, after: Option<u64>) -> Replay {
        let history = self.inner.history.lock().expect("event history lock");
        let expired = after.is_some_and(|after| {
            history
                .front()
                .is_some_and(|first| after.saturating_add(1) < first.id)
        });
        Replay {
            expired,
            events: history
                .iter()
                .filter(|event| after.is_none_or(|after| event.id > after))
                .cloned()
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    Running,
    Succeeded,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyOperation {
    pub id: Sha256Digest,
    pub plan_id: Sha256Digest,
    pub status: ApplyStatus,
    pub failure_code: Option<StableId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    pub status: ApplyStatus,
    pub failure_code: Option<StableId>,
}

pub trait PlanExecutor: Send + Sync + 'static {
    fn execute(&self, plan: &Plan, confirmation_id: &StableId) -> ExecutionResult;
}

struct UnavailableExecutor;

impl PlanExecutor for UnavailableExecutor {
    fn execute(&self, _plan: &Plan, _confirmation_id: &StableId) -> ExecutionResult {
        ExecutionResult {
            status: ApplyStatus::Failed,
            failure_code: Some(
                StableId::parse("executor_unavailable").expect("static stable identifier"),
            ),
        }
    }
}

#[derive(Clone)]
pub struct ControlPlane {
    inner: Arc<ControlPlaneInner>,
}

struct ControlPlaneInner {
    plans: std::sync::RwLock<BTreeMap<Sha256Digest, Plan>>,
    plan_store: Option<Arc<PlanStore>>,
    operations: std::sync::RwLock<BTreeMap<Sha256Digest, ApplyOperation>>,
    idempotency: std::sync::Mutex<BTreeMap<String, (Sha256Digest, Sha256Digest)>>,
    executor: Arc<dyn PlanExecutor>,
}

impl ControlPlane {
    pub fn new(executor: Arc<dyn PlanExecutor>) -> Self {
        Self {
            inner: Arc::new(ControlPlaneInner {
                plans: std::sync::RwLock::new(BTreeMap::new()),
                plan_store: None,
                operations: std::sync::RwLock::new(BTreeMap::new()),
                idempotency: std::sync::Mutex::new(BTreeMap::new()),
                executor,
            }),
        }
    }

    pub fn with_plan_store(executor: Arc<dyn PlanExecutor>, plan_store: Arc<PlanStore>) -> Self {
        Self {
            inner: Arc::new(ControlPlaneInner {
                plans: std::sync::RwLock::new(BTreeMap::new()),
                plan_store: Some(plan_store),
                operations: std::sync::RwLock::new(BTreeMap::new()),
                idempotency: std::sync::Mutex::new(BTreeMap::new()),
                executor,
            }),
        }
    }

    pub fn register_plan(&self, plan: Plan) -> Result<Plan, ControlError> {
        validate_control_plan(&plan)?;
        if let Some(store) = &self.inner.plan_store {
            store.persist(&plan)?;
        }
        let mut plans = self.inner.plans.write().expect("plan lock");
        if let Some(existing) = plans.get(&plan.id) {
            if existing == &plan {
                return Ok(existing.clone());
            }
            return Err(ControlError::PlanConflict);
        }
        plans.insert(plan.id.clone(), plan.clone());
        Ok(plan)
    }

    pub fn plan(&self, id: &Sha256Digest) -> Option<Plan> {
        if let Some(plan) = self.inner.plans.read().expect("plan lock").get(id) {
            return Some(plan.clone());
        }
        self.inner
            .plan_store
            .as_ref()
            .and_then(|store| store.load(id).ok())
    }

    pub fn operation(&self, id: &Sha256Digest) -> Option<ApplyOperation> {
        self.inner
            .operations
            .read()
            .expect("operation lock")
            .get(id)
            .cloned()
    }

    pub fn apply(
        &self,
        plan_id: &Sha256Digest,
        confirmation_id: &StableId,
        idempotency_key: &str,
    ) -> Result<(ApplyOperation, bool), ControlError> {
        let (operation, created, job) =
            self.reserve_apply(plan_id, confirmation_id, idempotency_key)?;
        if let Some(job) = job {
            return Ok((self.execute_job(job), created));
        }
        Ok((operation, created))
    }

    fn reserve_apply(
        &self,
        plan_id: &Sha256Digest,
        confirmation_id: &StableId,
        idempotency_key: &str,
    ) -> Result<(ApplyOperation, bool, Option<ExecutionJob>), ControlError> {
        if idempotency_key.is_empty()
            || idempotency_key.len() > 128
            || !idempotency_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(ControlError::InvalidIdempotencyKey);
        }
        let plan = self.plan(plan_id).ok_or(ControlError::PlanNotFound)?;
        let operation_id = digest_domain_json(
            "commonkit.control-operation.v1",
            &serde_json::json!({
                "planId": plan_id,
                "confirmationId": confirmation_id,
                "idempotencyKey": idempotency_key,
            }),
        )?;

        {
            let mut keys = self.inner.idempotency.lock().expect("idempotency lock");
            if let Some((bound_plan, existing_id)) = keys.get(idempotency_key) {
                if bound_plan != plan_id {
                    return Err(ControlError::IdempotencyConflict);
                }
                return self
                    .operation(existing_id)
                    .map(|operation| (operation, false, None))
                    .ok_or(ControlError::OperationNotFound);
            }
            let running = ApplyOperation {
                id: operation_id.clone(),
                plan_id: plan_id.clone(),
                status: ApplyStatus::Running,
                failure_code: None,
            };
            self.inner
                .operations
                .write()
                .expect("operation lock")
                .insert(operation_id.clone(), running);
            keys.insert(
                idempotency_key.into(),
                (plan_id.clone(), operation_id.clone()),
            );
        }
        Ok((
            self.operation(&operation_id)
                .expect("reserved operation exists"),
            true,
            Some(ExecutionJob {
                operation_id,
                plan,
                confirmation_id: confirmation_id.clone(),
            }),
        ))
    }

    fn execute_job(&self, job: ExecutionJob) -> ApplyOperation {
        let execution = self.inner.executor.execute(&job.plan, &job.confirmation_id);
        let completed = ApplyOperation {
            id: job.operation_id.clone(),
            plan_id: job.plan.id,
            status: execution.status,
            failure_code: execution.failure_code,
        };
        self.inner
            .operations
            .write()
            .expect("operation lock")
            .insert(job.operation_id, completed.clone());
        completed
    }
}

struct ExecutionJob {
    operation_id: Sha256Digest,
    plan: Plan,
    confirmation_id: StableId,
}

fn validate_control_plan(plan: &Plan) -> Result<(), ControlError> {
    let rebuilt = build_plan(PlanDraft {
        target_id: plan.target_id.clone(),
        desired_digest: plan.desired_digest.clone(),
        observed_digest: plan.observed_digest.clone(),
        policy_digest: plan.policy_digest.clone(),
        bindings: plan.bindings.clone(),
        operations: plan.operations.clone(),
    })
    .map_err(|_| ControlError::InvalidPlan)?;
    if rebuilt != *plan {
        return Err(ControlError::InvalidPlan);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("plan is invalid")]
    InvalidPlan,
    #[error("plan identity conflicts with registered content")]
    PlanConflict,
    #[error("plan was not found")]
    PlanNotFound,
    #[error("operation was not found")]
    OperationNotFound,
    #[error("idempotency key is invalid")]
    InvalidIdempotencyKey,
    #[error("idempotency key is already bound to another plan")]
    IdempotencyConflict,
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    PlanStore(#[from] PlanStoreError),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRequest {
    confirmed: bool,
    confirmation_id: StableId,
}

async fn register_plan(
    State(state): State<ApiState>,
    Json(plan): Json<Plan>,
) -> Result<(StatusCode, Json<Plan>), ApiError> {
    state
        .control
        .register_plan(plan)
        .map(|plan| (StatusCode::CREATED, Json(plan)))
        .map_err(ApiError::from)
}

async fn get_plan(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Plan>, ApiError> {
    let id = Sha256Digest::parse(id).map_err(|_| ApiError::bad_request("invalid_plan_id"))?;
    state
        .control
        .plan(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("plan_not_found"))
}

async fn apply_plan(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<ApplyRequest>,
) -> Result<(StatusCode, Json<ApplyOperation>), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("confirmation_required"));
    }
    let id = Sha256Digest::parse(id).map_err(|_| ApiError::bad_request("invalid_plan_id"))?;
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::bad_request("idempotency_key_required"))?;
    let (operation, created, job) = state
        .control
        .reserve_apply(&id, &request.confirmation_id, key)
        .map_err(ApiError::from)?;
    if created {
        state.events.publish(
            "operation.updated",
            serde_json::to_value(&operation).expect("operation serializes"),
        );
        let control = state.control.clone();
        let events = state.events.clone();
        tokio::task::spawn_blocking(move || {
            let completed = control.execute_job(job.expect("created operation has job"));
            events.publish(
                "operation.updated",
                serde_json::to_value(completed).expect("operation serializes"),
            );
        });
    }
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(operation),
    ))
}

async fn get_operation(
    State(state): State<ApiState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApplyOperation>, ApiError> {
    let id = Sha256Digest::parse(id).map_err(|_| ApiError::bad_request("invalid_operation_id"))?;
    state
        .control
        .operation(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("operation_not_found"))
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }

    fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
        }
    }

    fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
        }
    }

    fn internal(code: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
        }
    }
}

impl From<ControlError> for ApiError {
    fn from(error: ControlError) -> Self {
        match error {
            ControlError::PlanNotFound => Self::not_found("plan_not_found"),
            ControlError::OperationNotFound => Self::not_found("operation_not_found"),
            ControlError::IdempotencyConflict | ControlError::PlanConflict => {
                Self::conflict("conflict")
            }
            ControlError::InvalidIdempotencyKey => Self::bad_request("invalid_idempotency_key"),
            ControlError::InvalidPlan | ControlError::Contract(_) => {
                Self::bad_request("invalid_plan")
            }
            ControlError::PlanStore(_) => Self::internal("plan_store_failed"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "apiVersion": "commonkit.control/v1",
                "error": {
                    "code": self.code,
                    "message": self.code.replace('_', " "),
                    "retryable": false,
                }
            })),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub expired: bool,
    pub events: Vec<ControlEvent>,
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    after: Option<u64>,
}

async fn get_events(
    State(state): State<ApiState>,
    Query(query): Query<EventQuery>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let replay = state.events.replay_after(query.after);
    let initial = if replay.expired {
        vec![ControlEvent {
            id: replay
                .events
                .first()
                .map_or(0, |event| event.id.saturating_sub(1)),
            event: "resync_required".into(),
            data: serde_json::json!({}),
        }]
    } else {
        replay.events
    };
    let replay_stream = futures_util::stream::iter(initial.into_iter().map(to_sse));
    let live = BroadcastStream::new(state.events.inner.sender.subscribe()).filter_map(|message| {
        futures_util::future::ready(Some(match message {
            Ok(event) => to_sse(event),
            Err(BroadcastStreamRecvError::Lagged(_)) => {
                Ok(Event::default().event("resync_required").data("{}"))
            }
        }))
    });
    Sse::new(replay_stream.chain(live)).keep_alive(KeepAlive::default())
}

fn to_sse(event: ControlEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .id(event.id.to_string())
        .event(event.event)
        .json_data(event.data)
        .expect("JSON value serializes"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonDiscovery {
    pub schema_version: u32,
    pub pid: u32,
    pub started_at_unix_ms: u128,
    pub port: u16,
    pub token_id: String,
}

impl DaemonDiscovery {
    pub fn new(port: u16, token: &ControlToken) -> Result<Self, ServiceError> {
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            pid: std::process::id(),
            started_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| ServiceError::Clock)?
                .as_millis(),
            port,
            token_id: token.expose_for_client()[..12].into(),
        })
    }

    pub fn persist(&self, path: &Path) -> Result<(), ServiceError> {
        let parent = path.parent().ok_or(ServiceError::UnsafeDiscoveryPath)?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if temporary.exists() {
            std::fs::remove_file(&temporary)?;
        }
        let bytes = serde_json::to_vec(self)?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ServiceError::UnsafeDiscoveryPath);
            }
            std::fs::remove_file(path)?;
        }
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

pub async fn serve(
    address: SocketAddr,
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
) -> Result<(), ServiceError> {
    if !address.ip().is_loopback() {
        return Err(ServiceError::NonLoopbackBind(address.ip()));
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    axum::serve(listener, router(token, status, local.to_string())).await?;
    Ok(())
}

pub struct BoundServer {
    listener: tokio::net::TcpListener,
    application: Router,
    discovery_path: std::path::PathBuf,
    discovery: DaemonDiscovery,
}

impl BoundServer {
    pub async fn bind(
        address: SocketAddr,
        token: ControlToken,
        status: Arc<RwLock<ServiceStatus>>,
        events: EventHub,
        discovery_path: impl AsRef<Path>,
    ) -> Result<Self, ServiceError> {
        if !address.ip().is_loopback() {
            return Err(ServiceError::NonLoopbackBind(address.ip()));
        }
        let listener = tokio::net::TcpListener::bind(address).await?;
        let local = listener.local_addr()?;
        let discovery = DaemonDiscovery::new(local.port(), &token)?;
        discovery.persist(discovery_path.as_ref())?;
        let plan_root = discovery_path
            .as_ref()
            .parent()
            .ok_or(ServiceError::UnsafeDiscoveryPath)?
            .join("plans");
        let control = ControlPlane::with_plan_store(
            Arc::new(UnavailableExecutor),
            Arc::new(PlanStore::open(plan_root)?),
        );
        let application = router_with_control(token, status, local.to_string(), events, control);
        Ok(Self {
            listener,
            application,
            discovery_path: discovery_path.as_ref().to_path_buf(),
            discovery,
        })
    }

    pub fn discovery(&self) -> &DaemonDiscovery {
        &self.discovery
    }

    pub async fn run_until<F>(self, shutdown: F) -> Result<(), ServiceError>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let discovery_path = self.discovery_path.clone();
        let result = axum::serve(self.listener, self.application)
            .with_graceful_shutdown(shutdown)
            .await;
        match std::fs::remove_file(discovery_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        result?;
        Ok(())
    }
}

async fn get_status(State(state): State<ApiState>) -> Json<ServiceStatus> {
    Json(state.status.read().await.clone())
}

async fn get_diagnostics(
    State(state): State<ApiState>,
) -> Result<Json<DiagnosticBundle>, ApiError> {
    let status = state.status.read().await.clone();
    let generated_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::internal("clock_unavailable"))?
        .as_millis()
        .try_into()
        .map_err(|_| ApiError::internal("clock_overflow"))?;
    let scheduler_code = status
        .last_drift_error_code
        .as_deref()
        .and_then(|code| StableId::parse(code).ok());
    let bundle = DiagnosticBundle {
        schema_version: SchemaVersion(SCHEMA_VERSION),
        contract_version: CONTRACT_VERSION.into(),
        generated_at_unix_ms,
        runtime: RuntimeDiagnostic {
            version: env!("CARGO_PKG_VERSION").into(),
            operating_system: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
        },
        overall_state: diagnostic_state(status.state),
        components: vec![
            ComponentDiagnostic {
                id: StableId::parse("daemon").expect("static stable ID"),
                state: DiagnosticState::Healthy,
                code: None,
            },
            ComponentDiagnostic {
                id: StableId::parse("drift_scheduler").expect("static stable ID"),
                state: if scheduler_code.is_some() {
                    DiagnosticState::Degraded
                } else {
                    diagnostic_state(status.state)
                },
                code: scheduler_code,
            },
        ],
    };
    let value = serde_json::to_value(&bundle).map_err(|_| ApiError::internal("serialization"))?;
    assert_no_embedded_secrets(&value).map_err(|_| ApiError::internal("redaction_failed"))?;
    Ok(Json(bundle))
}

fn diagnostic_state(state: OverallState) -> DiagnosticState {
    match state {
        OverallState::Healthy => DiagnosticState::Healthy,
        OverallState::Drifted => DiagnosticState::Drifted,
        OverallState::Blocked => DiagnosticState::Blocked,
        OverallState::Applying => DiagnosticState::Applying,
        OverallState::Degraded => DiagnosticState::Degraded,
        OverallState::Offline => DiagnosticState::Offline,
    }
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn authenticate(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if headers.contains_key(header::ORIGIN) {
        return Err(StatusCode::FORBIDDEN);
    }
    if headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(state.authority.as_str())
    {
        return Err(StatusCode::MISDIRECTED_REQUEST);
    }
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !state.token.matches(authorization) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

fn read_token(file: &mut std::fs::File) -> Result<ControlToken, ServiceError> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(ServiceError::InvalidControlToken);
    }
    Ok(ControlToken(Arc::from(bytes)))
}

fn validate_token_file(path: &Path) -> Result<(), ServiceError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ServiceError::InsecureControlToken);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ServiceError::InsecureControlToken);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("local control API may bind only to a loopback address, not {0}")]
    NonLoopbackBind(IpAddr),
    #[error("control token file is malformed")]
    InvalidControlToken,
    #[error("control token must be a private regular file and cannot be a symlink")]
    InsecureControlToken,
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("daemon discovery path must have a parent directory")]
    UnsafeDiscoveryPath,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    PlanStore(#[from] PlanStoreError),
}

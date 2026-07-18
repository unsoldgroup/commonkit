//! Authenticated, loopback-only CommonKit local control API.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commonkit_adapters::{
    CredentialReadinessInspector, CredentialReference, FileAdapter,
    LocalCredentialReadinessInspector,
};
use commonkit_contracts::{
    CONTRACT_VERSION, ComponentDiagnostic, DiagnosticBundle, DiagnosticState, Plan, PlanBindings,
    ReceiptState, RuntimeDiagnostic, SCHEMA_VERSION, SchemaVersion, Sha256Digest, StableId,
    assert_no_embedded_secrets, digest_domain_json,
};
use commonkit_core::{PlanDraft, build_plan};
use commonkit_reconcile::{
    Adapter, PlanStore, PlanStoreError, ReceiptError, ReceiptStore, ReconcileOutcome, Reconciler,
};
use commonkit_relay::{
    DownstreamRequest, HttpUpstreamManager, PeerAddress, RelayAdapter, RelayLifecycleControl,
    RelayMutationInputs, RelayPlanRequest, RelayRuntime, ResolvedMcpDeclarations, converge_provider_mcp,
    plan_relay_operation,
};
use futures_util::StreamExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

mod production_domains;
pub use production_domains::{ProductionDomainError, ProductionDomainRegistry};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchedulerConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 900,
        }
    }
}

pub struct SchedulerStore {
    path: PathBuf,
}

impl SchedulerStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SchedulerError> {
        std::fs::create_dir_all(root.as_ref())?;
        make_private_directory(root.as_ref())?;
        Ok(Self {
            path: root.as_ref().join("scheduler.json"),
        })
    }

    fn load(&self) -> Result<SchedulerConfig, SchedulerError> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(SchedulerError::UnsafeState)
            }
            Ok(_) => Ok(serde_json::from_slice(&std::fs::read(&self.path)?)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SchedulerConfig::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, config: SchedulerConfig) -> Result<(), SchedulerError> {
        let bytes = serde_json::to_vec(&config)?;
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(Into::into)
    }
}

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("scheduler state is not a regular file")]
    UnsafeState,
    #[error("scheduler interval must be at least one second")]
    InvalidInterval,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

fn make_private_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct DriftScheduler<C> {
    checker: Arc<C>,
    status: Arc<RwLock<ServiceStatus>>,
    events: EventHub,
    configuration: std::sync::Mutex<SchedulerConfig>,
    store: Option<SchedulerStore>,
    running: AtomicBool,
}

impl<C: DriftChecker> DriftScheduler<C> {
    pub fn new(checker: Arc<C>, status: Arc<RwLock<ServiceStatus>>, events: EventHub) -> Self {
        Self {
            checker,
            status,
            events,
            configuration: std::sync::Mutex::new(SchedulerConfig::default()),
            store: None,
            running: AtomicBool::new(false),
        }
    }

    pub fn with_store(
        checker: Arc<C>,
        status: Arc<RwLock<ServiceStatus>>,
        events: EventHub,
        store: SchedulerStore,
    ) -> Result<Self, SchedulerError> {
        let configuration = store.load()?;
        Ok(Self {
            checker,
            status,
            events,
            configuration: std::sync::Mutex::new(configuration),
            store: Some(store),
            running: AtomicBool::new(false),
        })
    }

    pub fn configuration(&self) -> SchedulerConfig {
        *self
            .configuration
            .lock()
            .expect("scheduler configuration lock")
    }

    pub fn enable(&self, interval: Duration) -> Result<(), SchedulerError> {
        if interval.as_secs() == 0 {
            return Err(SchedulerError::InvalidInterval);
        }
        self.update_configuration(SchedulerConfig {
            enabled: true,
            interval_seconds: interval.as_secs(),
        })
    }

    pub fn disable(&self) -> Result<(), SchedulerError> {
        let mut next = self.configuration();
        next.enabled = false;
        self.update_configuration(next)
    }

    fn update_configuration(&self, next: SchedulerConfig) -> Result<(), SchedulerError> {
        if let Some(store) = &self.store {
            store.save(next)?;
        }
        *self
            .configuration
            .lock()
            .expect("scheduler configuration lock") = next;
        Ok(())
    }

    pub async fn check_now(&self) -> DriftResult {
        self.try_check_now().await.unwrap_or(DriftResult {
            state: self.status.read().await.state,
            code: Some("drift_check_in_progress".into()),
        })
    }

    pub async fn try_check_now(&self) -> Option<DriftResult> {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.events.publish(
                "drift.skipped",
                serde_json::json!({ "code": "overlap_suppressed" }),
            );
            return None;
        }
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.running);
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
        Some(result)
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
                    if self.configuration().enabled { self.try_check_now().await; }
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
    relay_runtime: Arc<ManagedRelayRuntime>,
}

struct ManagedRelayRuntime {
    token: String,
    config_path: PathBuf,
    runtime: std::sync::Mutex<Option<RelayRuntime<HttpUpstreamManager>>>,
}

impl ManagedRelayRuntime {
    fn new(token: String, config_path: PathBuf) -> Self {
        Self {
            token,
            config_path,
            runtime: std::sync::Mutex::new(None),
        }
    }

    fn handle(&self, authorization: Option<&str>, body: &Value) -> Result<Value, &'static str> {
        let expected = format!("Bearer {}", self.token);
        if authorization != Some(expected.as_str()) { return Err("unauthorized"); }
        let method = body.get("method").and_then(Value::as_str);
        match method {
            Some("initialize") => return Ok(serde_json::json!({
                "protocolVersion": body.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "commonkit-relay", "version": env!("CARGO_PKG_VERSION")}
            })),
            Some("notifications/initialized") | Some("ping") => return Ok(serde_json::json!({})),
            _ => {}
        }
        let request = match method {
            Some("tools/list") => DownstreamRequest::ListTools,
            Some("tools/call") => DownstreamRequest::CallTool {
                name: body.pointer("/params/name").and_then(Value::as_str).ok_or("invalid_request")?.into(),
                arguments: body.pointer("/params/arguments").cloned().unwrap_or_else(|| serde_json::json!({})),
            },
            _ => return Err("method_not_found"),
        };
        let runtime = self.runtime.lock().map_err(|_| "relay_unavailable")?;
        let runtime = runtime.as_ref().ok_or("relay_unconfigured")?;
        runtime.handle(PeerAddress::Loopback, authorization, request).map_err(|error| match error.code() {
            commonkit_relay::RelayErrorCode::Unauthorized => "unauthorized",
            commonkit_relay::RelayErrorCode::Forbidden => "forbidden",
            commonkit_relay::RelayErrorCode::ToolNotFound => "tool_not_found",
            commonkit_relay::RelayErrorCode::UpstreamUnavailable => "upstream_unavailable",
            commonkit_relay::RelayErrorCode::InvalidState => "relay_invalid_state",
        })
    }
}

async fn relay_mcp(
    State(runtime): State<Arc<ManagedRelayRuntime>>, headers: HeaderMap, Json(body): Json<Value>,
) -> Response {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let authorization = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok());
    match runtime.handle(authorization, &body) {
        Ok(result) => Json(serde_json::json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response(),
        Err(code) => {
            let status = match code { "unauthorized" => StatusCode::UNAUTHORIZED, "relay_unconfigured" => StatusCode::SERVICE_UNAVAILABLE, _ => StatusCode::BAD_REQUEST };
            (status, Json(serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32000,"message":code}}))).into_response()
        }
    }
}

fn relay_router(runtime: Arc<ManagedRelayRuntime>) -> Router {
    Router::new().route("/mcp", post(relay_mcp)).with_state(runtime)
}

fn default_relay_runtime(token: &ControlToken) -> Arc<ManagedRelayRuntime> {
    let config_path = commonkit_platform::AppPaths::discover()
        .map(|paths| paths.config.join("relay.json"))
        .unwrap_or_else(|_| PathBuf::from("relay.json"));
    Arc::new(ManagedRelayRuntime::new(
        token.expose_for_client().into(),
        config_path,
    ))
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

impl RelayLifecycleControl for ManagedRelayRuntime {
    fn reload(
        &self,
        desired: &commonkit_relay::RelayConfig,
    ) -> Result<(), commonkit_relay::RelayPlanError> {
        let manager = HttpUpstreamManager::new(Duration::from_secs(30))
            .map_err(|_| commonkit_relay::RelayPlanError::UnsafeState)?;
        let runtime = RelayRuntime::new(
            self.token.clone(),
            manager,
            desired.servers.clone(),
            unix_time_ms() as u64,
        )
        .map_err(|_| commonkit_relay::RelayPlanError::UnsafeState)?;
        *self
            .runtime
            .lock()
            .map_err(|_| commonkit_relay::RelayPlanError::UnsafeState)? = Some(runtime);
        Ok(())
    }

    fn restart(&self) -> Result<(), commonkit_relay::RelayPlanError> {
        let config = commonkit_relay::LegacyRelayReader::read(&self.config_path)
            .map_err(|_| commonkit_relay::RelayPlanError::UnsafeState)?;
        self.reload(&config)
    }
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
    let relay_runtime = default_relay_runtime(&token);
    router_with_control_and_relay(token, status, authority, events, control, relay_runtime)
}

fn router_with_control_and_relay(
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: impl Into<String>,
    events: EventHub,
    control: ControlPlane,
    relay_runtime: Arc<ManagedRelayRuntime>,
) -> Router {
    let state = ApiState {
        token,
        status,
        authority: authority.into(),
        events,
        control,
        relay_runtime,
    };
    Router::new()
        .route("/control/v1/status", get(get_status))
        .route("/control/v1/health", get(health))
        .route("/control/v1/events", get(get_events))
        .route("/control/v1/diagnostics", get(get_diagnostics))
        .route("/control/v1/compose", get(capability_unavailable))
        .route("/control/v1/explain", post(capability_unavailable))
        .route("/control/v1/sync/plan", post(sync_plan))
        .route("/control/v1/verify", post(verify_target))
        .route(
            "/control/v1/credentials/readiness",
            post(credentials_readiness),
        )
        .route("/control/v1/credentials/apply", post(credentials_apply))
        .route("/control/v1/credentials/verify", post(credentials_verify))
        .route(
            "/control/v1/schedule",
            get(schedule_status).post(update_schedule),
        )
        .route("/control/v1/relay", get(get_relay_status))
        .route("/control/v1/relay/reconcile", post(plan_relay_reconcile))
        .route("/control/v1/relay/restart", post(restart_relay))
        .route(
            "/control/v1/snapshots",
            get(snapshot_list).post(snapshot_create),
        )
        .route("/control/v1/snapshots/restore", post(snapshot_restore))
        .route("/control/v1/snapshots/promote", post(snapshot_promote))
        .route("/control/v1/rollback", post(rollback_run))
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

/// Executes approved local plans through CommonKit's durable reconciliation
/// state machine. Provider code is never invoked here: every operation must
/// already reference materialized artifacts reconstructable by the adapter.
pub struct LocalPlanExecutor {
    plan_store: Arc<PlanStore>,
    receipt_store: ReceiptStore,
    target_root: PathBuf,
    adapter_state: PathBuf,
    relay: Option<RelayExecutorConfig>,
    execution_lock: std::sync::Mutex<()>,
}

struct RelayExecutorConfig {
    live: PathBuf,
    state: PathBuf,
    lifecycle: Arc<dyn RelayLifecycleControl>,
}

impl LocalPlanExecutor {
    pub fn open(
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
        target_root: impl AsRef<Path>,
        adapter_state: impl AsRef<Path>,
    ) -> Result<Self, LocalExecutionError> {
        Ok(Self {
            plan_store,
            receipt_store: ReceiptStore::open(receipt_root)?,
            target_root: target_root.as_ref().to_path_buf(),
            adapter_state: adapter_state.as_ref().to_path_buf(),
            relay: None,
            execution_lock: std::sync::Mutex::new(()),
        })
    }

    pub fn with_relay(
        mut self, live: impl AsRef<Path>, state: impl AsRef<Path>,
        lifecycle: Arc<dyn RelayLifecycleControl>,
    ) -> Self {
        self.relay = Some(RelayExecutorConfig { live: live.as_ref().into(), state: state.as_ref().into(), lifecycle });
        self
    }

    fn adapters(&self) -> Result<Vec<Box<dyn Adapter>>, LocalExecutionError> {
        let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(FileAdapter::open(&self.target_root, &self.adapter_state)?)];
        if let Some(relay) = &self.relay {
            adapters.push(Box::new(
                RelayAdapter::open(stable_code("relay"), &relay.live, &relay.state)?
                    .with_lifecycle(relay.lifecycle.clone()),
            ));
        }
        Ok(adapters)
    }

    /// Recovers every non-terminal run from durable receipts without resolving
    /// providers, credentials, templates, or downloads again.
    pub fn recover_pending(
        &self,
    ) -> Result<Vec<(StableId, ReconcileOutcome)>, LocalExecutionError> {
        let _guard = self
            .execution_lock
            .lock()
            .map_err(|_| LocalExecutionError::Lock)?;
        let mut recovered = Vec::new();
        for run_id in self.receipt_store.run_ids()? {
            let receipt = self.receipt_store.load(run_id.clone())?;
            if matches!(
                receipt.receipt().state,
                ReceiptState::Succeeded
                    | ReceiptState::Canceled
                    | ReceiptState::RolledBack
                    | ReceiptState::RollbackFailed
            ) {
                continue;
            }
            let plan = self.plan_store.load(&receipt.receipt().plan_id)?;
            let outcome = self.recover_one(run_id.clone(), &plan)?;
            recovered.push((run_id, outcome));
        }
        Ok(recovered)
    }

    fn recover_one(
        &self,
        run_id: StableId,
        plan: &Plan,
    ) -> Result<ReconcileOutcome, LocalExecutionError> {
        let mut adapters = self.adapters()?;
        Ok(Reconciler::with_store(&self.receipt_store).recover_run(run_id, plan, &mut adapters)?)
    }

    fn execute_durable(
        &self,
        plan: &Plan,
        confirmation_id: &StableId,
    ) -> Result<ReconcileOutcome, LocalExecutionError> {
        let durable = self.plan_store.load(&plan.id)?;
        if durable != *plan {
            return Err(LocalExecutionError::PlanMismatch);
        }
        let run_id = durable_run_id(&plan.id, confirmation_id)?;
        match self.receipt_store.load(run_id.clone()) {
            Ok(receipt) => match receipt.receipt().state {
                ReceiptState::Succeeded => return Ok(ReconcileOutcome::Succeeded),
                ReceiptState::Canceled => return Ok(ReconcileOutcome::Canceled),
                ReceiptState::RolledBack => return Ok(ReconcileOutcome::RolledBack),
                ReceiptState::RollbackFailed => return Ok(ReconcileOutcome::RollbackFailed),
                _ => return self.recover_one(run_id, &durable),
            },
            Err(ReceiptError::NotFound(_)) => {}
            Err(ReceiptError::Io(ref error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut adapters = self.adapters()?;
        Ok(Reconciler::with_store(&self.receipt_store).execute(&durable, run_id, &mut adapters)?)
    }
}

impl PlanExecutor for LocalPlanExecutor {
    fn execute(&self, plan: &Plan, confirmation_id: &StableId) -> ExecutionResult {
        let result = self
            .execution_lock
            .lock()
            .map_err(|_| LocalExecutionError::Lock)
            .and_then(|_guard| self.execute_durable(plan, confirmation_id));
        match result {
            Ok(ReconcileOutcome::Succeeded) => ExecutionResult {
                status: ApplyStatus::Succeeded,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RolledBack | ReconcileOutcome::Canceled) => ExecutionResult {
                status: ApplyStatus::RolledBack,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RollbackFailed) => ExecutionResult {
                status: ApplyStatus::Failed,
                failure_code: Some(stable_code("rollback_failed")),
            },
            Err(error) => ExecutionResult {
                status: ApplyStatus::Failed,
                failure_code: Some(error.code()),
            },
        }
    }
}

/// Durable executor for relay-only plans. It reconstructs both transaction
/// artifacts and the live runtime boundary after a process restart.
pub struct RelayPlanExecutor {
    plan_store: Arc<PlanStore>,
    receipt_store: ReceiptStore,
    live: PathBuf,
    state: PathBuf,
    lifecycle: Arc<dyn RelayLifecycleControl>,
    execution_lock: std::sync::Mutex<()>,
}

impl RelayPlanExecutor {
    pub fn open(
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
        live: impl AsRef<Path>,
        state: impl AsRef<Path>,
        lifecycle: Arc<dyn RelayLifecycleControl>,
    ) -> Result<Self, LocalExecutionError> {
        Ok(Self {
            plan_store,
            receipt_store: ReceiptStore::open(receipt_root)?,
            live: live.as_ref().into(),
            state: state.as_ref().into(),
            lifecycle,
            execution_lock: std::sync::Mutex::new(()),
        })
    }

    fn adapter(&self) -> Result<RelayAdapter, LocalExecutionError> {
        Ok(
            RelayAdapter::open(stable_code("relay"), &self.live, &self.state)?
                .with_lifecycle(self.lifecycle.clone()),
        )
    }

    fn execute_durable(
        &self,
        plan: &Plan,
        confirmation_id: &StableId,
    ) -> Result<ReconcileOutcome, LocalExecutionError> {
        if plan
            .operations
            .iter()
            .any(|operation| operation.adapter_id.as_str() != "relay")
        {
            return Err(LocalExecutionError::PlanMismatch);
        }
        let durable = self.plan_store.load(&plan.id)?;
        if durable != *plan {
            return Err(LocalExecutionError::PlanMismatch);
        }
        let run_id = durable_run_id(&plan.id, confirmation_id)?;
        let mut adapter: Vec<Box<dyn Adapter>> = vec![Box::new(self.adapter()?)];
        match self.receipt_store.load(run_id.clone()) {
            Ok(receipt) => match receipt.receipt().state {
                ReceiptState::Succeeded => Ok(ReconcileOutcome::Succeeded),
                ReceiptState::Canceled => Ok(ReconcileOutcome::Canceled),
                ReceiptState::RolledBack => Ok(ReconcileOutcome::RolledBack),
                ReceiptState::RollbackFailed => Ok(ReconcileOutcome::RollbackFailed),
                _ => Ok(Reconciler::with_store(&self.receipt_store).recover_run(
                    run_id,
                    &durable,
                    &mut adapter,
                )?),
            },
            Err(ReceiptError::NotFound(_)) => Ok(Reconciler::with_store(&self.receipt_store)
                .execute(&durable, run_id, &mut adapter)?),
            Err(ReceiptError::Io(ref error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Reconciler::with_store(&self.receipt_store).execute(
                    &durable,
                    run_id,
                    &mut adapter,
                )?)
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl PlanExecutor for RelayPlanExecutor {
    fn execute(&self, plan: &Plan, confirmation_id: &StableId) -> ExecutionResult {
        let result = self
            .execution_lock
            .lock()
            .map_err(|_| LocalExecutionError::Lock)
            .and_then(|_guard| self.execute_durable(plan, confirmation_id));
        match result {
            Ok(ReconcileOutcome::Succeeded) => ExecutionResult {
                status: ApplyStatus::Succeeded,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RolledBack | ReconcileOutcome::Canceled) => ExecutionResult {
                status: ApplyStatus::RolledBack,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RollbackFailed) => ExecutionResult {
                status: ApplyStatus::Failed,
                failure_code: Some(stable_code("rollback_failed")),
            },
            Err(_) => ExecutionResult {
                status: ApplyStatus::Failed,
                failure_code: Some(stable_code("relay_execution_failed")),
            },
        }
    }
}

fn durable_run_id(
    plan_id: &Sha256Digest,
    confirmation_id: &StableId,
) -> Result<StableId, commonkit_contracts::ContractError> {
    let digest = digest_domain_json(
        "commonkit.local-run.v1",
        &serde_json::json!({ "planId": plan_id, "confirmationId": confirmation_id }),
    )?;
    StableId::parse(format!("run-{}", &digest.as_str()[7..63]))
}

fn stable_code(value: &str) -> StableId {
    StableId::parse(value).expect("static failure code")
}

#[derive(Debug, Error)]
pub enum LocalExecutionError {
    #[error("the supplied plan does not match durable approved content")]
    PlanMismatch,
    #[error("the local execution lock is poisoned")]
    Lock,
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    PlanStore(#[from] PlanStoreError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error(transparent)]
    Reconcile(#[from] commonkit_reconcile::ReconcileError),
    #[error(transparent)]
    Adapter(#[from] commonkit_adapters::FileAdapterError),
    #[error(transparent)]
    Relay(#[from] commonkit_relay::RelayPlanError),
}

impl LocalExecutionError {
    fn code(&self) -> StableId {
        stable_code(match self {
            Self::PlanMismatch => "stale_plan",
            Self::Lock => "executor_unavailable",
            Self::PlanStore(_) => "plan_invalid",
            Self::Receipt(_) => "receipt_invalid",
            Self::Reconcile(_) => "reconcile_failed",
            Self::Adapter(_) => "adapter_unavailable",
            Self::Relay(_) => "relay_unavailable",
            Self::Contract(_) => "contract_invalid",
        })
    }
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

pub trait SyncDomain: Send + Sync + 'static {
    fn plan(&self, request: Value) -> Result<Value, DomainFailure>;
    fn verify(&self, request: Value) -> Result<Value, DomainFailure>;
    fn rollback(&self, request: Value) -> Result<Value, DomainFailure>;
}

pub trait CredentialDomain: Send + Sync + 'static {
    fn apply(&self, request: Value) -> Result<Value, DomainFailure>;
    fn verify(&self, request: Value) -> Result<Value, DomainFailure>;
}

pub trait SnapshotDomain: Send + Sync + 'static {
    fn create(&self, request: Value) -> Result<Value, DomainFailure>;
    fn list(&self) -> Result<Value, DomainFailure>;
    fn restore(&self, request: Value) -> Result<Value, DomainFailure>;
    fn promote(&self, request: Value) -> Result<Value, DomainFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainFailure {
    InvalidRequest,
    StalePlan,
    VerificationFailed,
    OperationFailed,
}

#[derive(Clone, Default)]
pub struct HeadlessDomainRegistry {
    pub sync: Option<Arc<dyn SyncDomain>>,
    pub credentials: Option<Arc<dyn CredentialDomain>>,
    pub snapshots: Option<Arc<dyn SnapshotDomain>>,
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
    scheduler_store: std::sync::RwLock<Option<Arc<SchedulerStore>>>,
    domains: std::sync::RwLock<HeadlessDomainRegistry>,
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
                scheduler_store: std::sync::RwLock::new(None),
                domains: std::sync::RwLock::new(HeadlessDomainRegistry::default()),
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
                scheduler_store: std::sync::RwLock::new(None),
                domains: std::sync::RwLock::new(HeadlessDomainRegistry::default()),
            }),
        }
    }

    pub fn set_scheduler_store(&self, store: Arc<SchedulerStore>) {
        *self
            .inner
            .scheduler_store
            .write()
            .expect("scheduler store lock") = Some(store);
    }

    pub fn set_headless_domains(&self, domains: HeadlessDomainRegistry) {
        *self.inner.domains.write().expect("domain registry lock") = domains;
    }

    pub fn scheduler_configuration(&self) -> Result<SchedulerConfig, SchedulerError> {
        self.inner
            .scheduler_store
            .read()
            .expect("scheduler store lock")
            .as_ref()
            .map_or_else(|| Ok(SchedulerConfig::default()), |store| store.load())
    }

    pub fn update_scheduler(
        &self,
        config: SchedulerConfig,
    ) -> Result<SchedulerConfig, SchedulerError> {
        if config.enabled && config.interval_seconds == 0 {
            return Err(SchedulerError::InvalidInterval);
        }
        if let Some(store) = self
            .inner
            .scheduler_store
            .read()
            .expect("scheduler store lock")
            .as_ref()
        {
            store.save(config)?;
        }
        Ok(config)
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
    message: Option<&'static str>,
}

impl ApiError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: None,
        }
    }

    fn not_found(code: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            message: None,
        }
    }

    fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message: None,
        }
    }

    fn internal(code: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message: None,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "capability_unavailable",
            message: Some(
                "This capability is not implemented by the current CommonKit service; update the daemon or enable the required provider",
            ),
        }
    }

    fn unavailable_code(code: &'static str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message: Some(
                "The required CommonKit domain is not configured; no operation was performed",
            ),
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
                    "message": self.message.map(str::to_owned).unwrap_or_else(|| self.code.replace('_', " ")),
                    "retryable": false,
                }
            })),
        )
            .into_response()
    }
}

async fn capability_unavailable() -> ApiError {
    ApiError::unavailable()
}

fn require_consent(value: &Value) -> Result<(), ApiError> {
    assert_domain_request_safe(value)?;
    if value.get("confirmed").and_then(Value::as_bool) != Some(true) {
        return Err(ApiError::conflict("confirmation_required"));
    }
    let confirmation = value
        .get("confirmationId")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("confirmation_id_required"))?;
    StableId::parse(confirmation).map_err(|_| ApiError::bad_request("invalid_confirmation_id"))?;
    Ok(())
}

fn assert_domain_request_safe(value: &Value) -> Result<(), ApiError> {
    let mut portable = value.clone();
    if let Some(object) = portable.as_object_mut() {
        object.remove("confirmationId");
        object.remove("idempotencyKey");
    }
    assert_no_embedded_secrets(&portable)
        .map_err(|_| ApiError::bad_request("embedded_secret_rejected"))
}

fn safe_domain_result(result: Result<Value, DomainFailure>) -> Result<Json<Value>, ApiError> {
    let value = result.map_err(domain_error)?;
    assert_no_embedded_secrets(&value).map_err(|_| ApiError::internal("unsafe_domain_response"))?;
    Ok(Json(value))
}

fn domain_error(error: DomainFailure) -> ApiError {
    match error {
        DomainFailure::InvalidRequest => ApiError::bad_request("invalid_domain_request"),
        DomainFailure::StalePlan => ApiError::conflict("stale_plan"),
        DomainFailure::VerificationFailed => ApiError::conflict("verification_failed"),
        DomainFailure::OperationFailed => ApiError::internal("domain_operation_failed"),
    }
}

async fn sync_plan(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .sync
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("sync_domain_unconfigured"))?;
    safe_domain_result(domain.plan(request))
}

async fn verify_target(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    assert_domain_request_safe(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .sync
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("sync_domain_unconfigured"))?;
    safe_domain_result(domain.verify(request))
}

async fn rollback_run(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .sync
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("sync_domain_unconfigured"))?;
    safe_domain_result(domain.rollback(request))
}

async fn credentials_apply(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .credentials
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("credential_domain_unconfigured"))?;
    safe_domain_result(domain.apply(request))
}

async fn credentials_verify(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    assert_domain_request_safe(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .credentials
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("credential_domain_unconfigured"))?;
    safe_domain_result(domain.verify(request))
}

async fn snapshot_create(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .snapshots
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("snapshot_domain_unconfigured"))?;
    safe_domain_result(domain.create(request))
}

async fn snapshot_list(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .snapshots
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("snapshot_domain_unconfigured"))?;
    safe_domain_result(domain.list())
}

async fn snapshot_restore(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .snapshots
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("snapshot_domain_unconfigured"))?;
    safe_domain_result(domain.restore(request))
}

async fn snapshot_promote(
    State(state): State<ApiState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_consent(&request)?;
    let domain = state
        .control
        .inner
        .domains
        .read()
        .expect("domain registry lock")
        .snapshots
        .clone()
        .ok_or_else(|| ApiError::unavailable_code("snapshot_domain_unconfigured"))?;
    safe_domain_result(domain.promote(request))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialReadinessRequest {
    references: Vec<String>,
}

async fn credentials_readiness(
    Json(request): Json<CredentialReadinessRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.references.len() > 256 {
        return Err(ApiError::bad_request("too_many_credential_references"));
    }
    let inspector = LocalCredentialReadinessInspector;
    let mut results = Vec::with_capacity(request.references.len());
    for value in request.references {
        let reference = CredentialReference::parse(&value)
            .map_err(|_| ApiError::bad_request("invalid_credential_reference"))?;
        results.push(
            serde_json::json!({ "reference": value, "readiness": inspector.inspect(&reference) }),
        );
    }
    Ok(Json(serde_json::json!({ "credentials": results })))
}

async fn schedule_status(State(state): State<ApiState>) -> Result<Json<SchedulerConfig>, ApiError> {
    state
        .control
        .scheduler_configuration()
        .map(Json)
        .map_err(|_| ApiError::internal("scheduler_state_invalid"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleRequest {
    enabled: bool,
    interval_seconds: Option<u64>,
    confirmed: bool,
    confirmation_id: StableId,
}

async fn update_schedule(
    State(state): State<ApiState>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<SchedulerConfig>, ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("confirmation_required"));
    }
    let _confirmation_id = request.confirmation_id;
    let current = state
        .control
        .scheduler_configuration()
        .map_err(|_| ApiError::internal("scheduler_state_invalid"))?;
    let config = SchedulerConfig {
        enabled: request.enabled,
        interval_seconds: request.interval_seconds.unwrap_or(current.interval_seconds),
    };
    state
        .control
        .update_scheduler(config)
        .map(Json)
        .map_err(|error| match error {
            SchedulerError::InvalidInterval => ApiError::bad_request("invalid_schedule_interval"),
            _ => ApiError::internal("scheduler_state_invalid"),
        })
}

async fn get_relay_status() -> Result<Json<Value>, ApiError> {
    let paths = commonkit_platform::AppPaths::discover()
        .map_err(|_| ApiError::internal("relay_paths_unavailable"))?;
    let path = paths.config.join("relay.json");
    let config = match commonkit_relay::LegacyRelayReader::read(&path) {
        Ok(config) => Some(config),
        Err(commonkit_relay::LegacyRelayError::Missing) => None,
        Err(_) => return Err(ApiError::internal("relay_config_invalid")),
    };
    Ok(Json(serde_json::json!({
        "configured": config.is_some(),
        "listen": config.as_ref().map(|value| &value.listen),
        "servers": config.as_ref().map(|value| value.servers.len()).unwrap_or(0),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelayRestartRequest {
    confirmed: bool,
}

async fn restart_relay(
    State(state): State<ApiState>,
    Json(request): Json<RelayRestartRequest>,
) -> Result<Json<Value>, ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("confirmation_required"));
    }
    state
        .relay_runtime
        .restart()
        .map_err(|_| ApiError::conflict("relay_unconfigured"))?;
    state
        .events
        .publish("relay.restarted", serde_json::json!({}));
    Ok(Json(serde_json::json!({"status": "restarted"})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelayReconcileRequest {
    confirmed: bool,
    confirmation_id: StableId,
    idempotency_key: String,
    resolved: ResolvedMcpDeclarations,
    target_identity_digest: Sha256Digest,
    composed_loadout_digest: Sha256Digest,
    provider_inputs_digest: Sha256Digest,
    policy_digest: Sha256Digest,
    ownership_map_digest: Sha256Digest,
    artifact_set_digest: Sha256Digest,
}

async fn plan_relay_reconcile(
    State(state): State<ApiState>,
    Json(request): Json<RelayReconcileRequest>,
) -> Result<(StatusCode, Json<Plan>), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("confirmation_required"));
    }
    if request.idempotency_key.is_empty() || request.idempotency_key.len() > 128 {
        return Err(ApiError::bad_request("invalid_idempotency_key"));
    }
    let _confirmation_id = request.confirmation_id;
    let converged = converge_provider_mcp(request.resolved)
        .map_err(|_| ApiError::bad_request("relay_declarations_invalid"))?;
    let paths = commonkit_platform::AppPaths::discover()
        .map_err(|_| ApiError::internal("relay_paths_unavailable"))?;
    paths
        .create_private_roots()
        .map_err(|_| ApiError::internal("relay_paths_unavailable"))?;
    let mut adapter = RelayAdapter::open(
        StableId::parse("relay").expect("static stable identifier"),
        paths.config.join("relay.json"),
        paths.state.join("relay"),
    )
    .map_err(|_| ApiError::internal("relay_state_unavailable"))?;
    let operation = plan_relay_operation(
        &mut adapter,
        RelayPlanRequest {
            desired: converged.relay.clone(),
            inputs: RelayMutationInputs {
                provider_inputs_digest: request.provider_inputs_digest.clone(),
                policy_digest: request.policy_digest.clone(),
                target_digest: request.target_identity_digest.clone(),
            },
        },
    )
    .map_err(|_| ApiError::internal("relay_plan_failed"))?;
    let desired_digest = digest_domain_json("commonkit.relay-desired.v1", &converged.relay)
        .map_err(|_| ApiError::internal("relay_plan_failed"))?;
    let observed_digest = operation
        .as_ref()
        .and_then(|value| value.before_digest.clone())
        .unwrap_or_else(|| desired_digest.clone());
    let plan = build_plan(PlanDraft {
        target_id: StableId::parse("local").expect("static stable identifier"),
        desired_digest,
        observed_digest,
        policy_digest: request.policy_digest.clone(),
        bindings: PlanBindings {
            target_identity_digest: request.target_identity_digest,
            composed_loadout_digest: request.composed_loadout_digest,
            provider_inputs_digest: request.provider_inputs_digest,
            ownership_map_digest: request.ownership_map_digest,
            artifact_set_digest: request.artifact_set_digest,
        },
        operations: operation.into_iter().collect(),
    })
    .map_err(|_| ApiError::internal("relay_plan_failed"))?;
    let plan = state.control.register_plan(plan)?;
    Ok((StatusCode::CREATED, Json(plan)))
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
    relay: Option<(tokio::net::TcpListener, Router)>,
    relay_address: Option<SocketAddr>,
}

impl BoundServer {
    pub async fn bind(
        address: SocketAddr,
        token: ControlToken,
        status: Arc<RwLock<ServiceStatus>>,
        events: EventHub,
        discovery_path: impl AsRef<Path>,
    ) -> Result<Self, ServiceError> {
        Self::bind_inner(address, None, token, status, events, discovery_path).await
    }

    pub async fn bind_with_relay_address(
        address: SocketAddr,
        relay_address: SocketAddr,
        token: ControlToken,
        status: Arc<RwLock<ServiceStatus>>,
        events: EventHub,
        discovery_path: impl AsRef<Path>,
    ) -> Result<Self, ServiceError> {
        if !relay_address.ip().is_loopback() {
            return Err(ServiceError::NonLoopbackBind(relay_address.ip()));
        }
        Self::bind_inner(address, Some(relay_address), token, status, events, discovery_path).await
    }

    async fn bind_inner(
        address: SocketAddr,
        relay_address: Option<SocketAddr>,
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
        let plan_store = Arc::new(PlanStore::open(plan_root)?);
        let paths = commonkit_platform::AppPaths::discover()
            .map_err(|_| ServiceError::UnsafeDiscoveryPath)?;
        paths
            .create_private_roots()
            .map_err(|_| ServiceError::UnsafeDiscoveryPath)?;
        let relay_runtime = Arc::new(ManagedRelayRuntime::new(
            token.expose_for_client().into(),
            paths.config.join("relay.json"),
        ));
        let executor = LocalPlanExecutor::open(
            plan_store.clone(), &paths.receipts, &paths.config, paths.state.join("filesystem"),
        ).map_err(|_| ServiceError::UnsafeDiscoveryPath)?
        .with_relay(paths.config.join("relay.json"), paths.state.join("relay"), relay_runtime.clone());
        let control = ControlPlane::with_plan_store(Arc::new(executor), plan_store.clone());
        let production_domains = ProductionDomainRegistry::load_optional(
            &paths.config.join("headless.json"),
            plan_store,
            paths.receipts.clone(),
        )?;
        control.set_headless_domains(production_domains.into_headless());
        control.set_scheduler_store(Arc::new(
            SchedulerStore::open(
                discovery_path
                    .as_ref()
                    .parent()
                    .ok_or(ServiceError::UnsafeDiscoveryPath)?
                    .join("scheduler"),
            )
            .map_err(|_| ServiceError::UnsafeDiscoveryPath)?,
        ));
        let application = router_with_control_and_relay(
            token,
            status,
            local.to_string(),
            events,
            control,
            relay_runtime.clone(),
        );
        let relay = match relay_address {
            Some(address) => Some((tokio::net::TcpListener::bind(address).await?, relay_router(relay_runtime))),
            None => None,
        };
        let relay_address = relay.as_ref().and_then(|(listener, _)| listener.local_addr().ok());
        Ok(Self {
            listener,
            application,
            discovery_path: discovery_path.as_ref().to_path_buf(),
            discovery,
            relay,
            relay_address,
        })
    }

    pub fn discovery(&self) -> &DaemonDiscovery {
        &self.discovery
    }

    pub fn relay_address(&self) -> Option<SocketAddr> { self.relay_address }

    pub async fn run_until<F>(self, shutdown: F) -> Result<(), ServiceError>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let discovery_path = self.discovery_path.clone();
        let relay_task = self.relay.map(|(listener, application)| tokio::spawn(async move {
            axum::serve(listener, application).await
        }));
        let result = axum::serve(self.listener, self.application)
            .with_graceful_shutdown(shutdown)
            .await;
        if let Some(task) = relay_task { task.abort(); }
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
    #[error(transparent)]
    ProductionDomains(#[from] ProductionDomainError),
}

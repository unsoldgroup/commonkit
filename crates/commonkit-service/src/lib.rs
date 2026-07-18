//! Authenticated, loopback-only CommonKit local control API.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use commonkit_contracts::{CONTRACT_VERSION, SCHEMA_VERSION};
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
    let state = ApiState {
        token,
        status,
        authority: authority.into(),
        events,
    };
    Router::new()
        .route("/control/v1/status", get(get_status))
        .route("/control/v1/health", get(health))
        .route("/control/v1/events", get(get_events))
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
        let application = router_with_events(token, status, local.to_string(), events);
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
}

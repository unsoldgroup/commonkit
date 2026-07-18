//! Authenticated, loopback-only CommonKit local control API.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use commonkit_contracts::{CONTRACT_VERSION, SCHEMA_VERSION};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::RwLock;

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
}

pub fn router(
    token: ControlToken,
    status: Arc<RwLock<ServiceStatus>>,
    authority: impl Into<String>,
) -> Router {
    let state = ApiState {
        token,
        status,
        authority: authority.into(),
    };
    Router::new()
        .route("/control/v1/status", get(get_status))
        .route("/control/v1/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use commonkit_contracts::Sha256Digest;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStdioLimits {
    pub startup_timeout_ms: u64,
    pub call_timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_restarts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStdioUpstream {
    executable: PathBuf,
    executable_digest: Sha256Digest,
    arguments: Vec<String>,
    environment_references: BTreeMap<String, String>,
    limits: LocalStdioLimits,
}

impl LocalStdioUpstream {
    pub fn new(
        executable: PathBuf,
        executable_digest: Sha256Digest,
        arguments: Vec<String>,
        environment_references: BTreeMap<String, String>,
        limits: LocalStdioLimits,
    ) -> Result<Self, LocalStdioValidationError> {
        if !executable.is_absolute() {
            return Err(LocalStdioValidationError::ExecutableMustBeAbsolute);
        }
        if arguments.iter().any(|argument| argument.contains('\0')) {
            return Err(LocalStdioValidationError::InvalidArgument);
        }
        if environment_references.iter().any(|(key, value)| {
            key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                || !is_secret_reference(value)
        }) {
            return Err(LocalStdioValidationError::LiteralEnvironmentSecret);
        }
        if limits.startup_timeout_ms == 0
            || limits.startup_timeout_ms > 60_000
            || limits.call_timeout_ms == 0
            || limits.call_timeout_ms > 300_000
            || limits.max_request_bytes == 0
            || limits.max_request_bytes > 256 * 1024
            || limits.max_response_bytes == 0
            || limits.max_response_bytes > 1024 * 1024
            || limits.max_restarts > 10
        {
            return Err(LocalStdioValidationError::InvalidLimits);
        }
        Ok(Self {
            executable,
            executable_digest,
            arguments,
            environment_references,
            limits,
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn executable_digest(&self) -> &Sha256Digest {
        &self.executable_digest
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn environment_references(&self) -> &BTreeMap<String, String> {
        &self.environment_references
    }

    pub fn limits(&self) -> LocalStdioLimits {
        self.limits
    }
}

fn is_secret_reference(value: &str) -> bool {
    ["env:", "secret:", "bws:", "keychain:", "vault:"]
        .iter()
        .any(|prefix| value.starts_with(prefix) && value.len() > prefix.len())
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LocalStdioValidationError {
    #[error("local stdio executable must be an absolute allowlisted path")]
    ExecutableMustBeAbsolute,
    #[error("local stdio arguments are invalid")]
    InvalidArgument,
    #[error("local stdio environment contains a literal secret")]
    LiteralEnvironmentSecret,
    #[error("local stdio lifecycle limits are invalid")]
    InvalidLimits,
}

pub struct ResolvedSecret(String);

impl ResolvedSecret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for ResolvedSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub trait LocalSecretResolver: Send + Sync + 'static {
    fn resolve(&self, reference: &str) -> Result<ResolvedSecret, LocalStdioError>;
}

struct RunningProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RunningProcess {
    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

struct ProcessState {
    process: Option<RunningProcess>,
    restart_count: u32,
}

/// Persistent, serialized stdio MCP transport. A call is never retried after
/// bytes have been written because doing so could duplicate a mutation.
pub struct LocalStdioProcessManager {
    upstream: LocalStdioUpstream,
    secrets: Arc<dyn LocalSecretResolver>,
    state: Mutex<ProcessState>,
}

impl LocalStdioProcessManager {
    pub fn new(upstream: LocalStdioUpstream, secrets: Arc<dyn LocalSecretResolver>) -> Self {
        Self {
            upstream,
            secrets,
            state: Mutex::new(ProcessState {
                process: None,
                restart_count: 0,
            }),
        }
    }

    pub async fn call(&self, request: &[u8]) -> Result<Vec<u8>, LocalStdioError> {
        if request.is_empty() || request.len() > self.upstream.limits.max_request_bytes {
            return Err(LocalStdioError::RequestTooLarge);
        }
        if request.contains(&b'\n') {
            return Err(LocalStdioError::InvalidFrame);
        }
        let mut state = self.state.lock().await;
        if state.process.is_none() {
            if state.restart_count > self.upstream.limits.max_restarts {
                return Err(LocalStdioError::RestartBudgetExhausted);
            }
            state.process = Some(
                timeout(
                    Duration::from_millis(self.upstream.limits.startup_timeout_ms),
                    self.spawn(),
                )
                .await
                .map_err(|_| LocalStdioError::StartupTimeout)??,
            );
            state.restart_count = state.restart_count.saturating_add(1);
        }

        let process = state.process.as_mut().expect("process initialized");
        let result = timeout(
            Duration::from_millis(self.upstream.limits.call_timeout_ms),
            exchange(process, request, self.upstream.limits.max_response_bytes),
        )
        .await;
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                if let Some(process) = state.process.take() {
                    process.stop().await;
                }
                Err(error)
            }
            Err(_) => {
                if let Some(process) = state.process.take() {
                    process.stop().await;
                }
                Err(LocalStdioError::CallTimeout)
            }
        }
    }

    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        if let Some(process) = state.process.take() {
            process.stop().await;
        }
    }

    async fn spawn(&self) -> Result<RunningProcess, LocalStdioError> {
        verify_executable(&self.upstream)?;
        let mut resolved = Vec::new();
        for (name, reference) in &self.upstream.environment_references {
            resolved.push((name, self.secrets.resolve(reference)?));
        }
        let mut command = Command::new(&self.upstream.executable);
        command
            .args(&self.upstream.arguments)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (name, value) in &resolved {
            command.env(name, value.expose());
        }
        let mut child = command.spawn().map_err(LocalStdioError::Spawn)?;
        let stdin = child.stdin.take().ok_or(LocalStdioError::MissingPipe)?;
        let stdout = child.stdout.take().ok_or(LocalStdioError::MissingPipe)?;
        Ok(RunningProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
}

async fn exchange(
    process: &mut RunningProcess,
    request: &[u8],
    max_response_bytes: usize,
) -> Result<Vec<u8>, LocalStdioError> {
    process.stdin.write_all(request).await?;
    process.stdin.write_all(b"\n").await?;
    process.stdin.flush().await?;
    let mut response = Vec::new();
    loop {
        let available = process.stdout.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if response.len().saturating_add(consumed) > max_response_bytes.saturating_add(1) {
            return Err(LocalStdioError::ResponseTooLarge);
        }
        response.extend_from_slice(&available[..consumed]);
        process.stdout.consume(consumed);
        if response.last() == Some(&b'\n') {
            break;
        }
    }
    if response.last() == Some(&b'\n') {
        response.pop();
        if response.last() == Some(&b'\r') {
            response.pop();
        }
    }
    if response.is_empty() {
        return Err(LocalStdioError::Closed);
    }
    Ok(response)
}

fn verify_executable(upstream: &LocalStdioUpstream) -> Result<(), LocalStdioError> {
    let metadata = std::fs::symlink_metadata(&upstream.executable)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LocalStdioError::UnsafeExecutable);
    }
    let actual = Sha256Digest::parse(format!(
        "sha256:{:x}",
        Sha256::digest(std::fs::read(&upstream.executable)?)
    ))
    .map_err(|_| LocalStdioError::Digest)?;
    if actual != upstream.executable_digest {
        return Err(LocalStdioError::ExecutableDigestMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum LocalStdioError {
    #[error("local stdio request exceeds its configured bound")]
    RequestTooLarge,
    #[error("local stdio request is not one newline-delimited frame")]
    InvalidFrame,
    #[error("local stdio response exceeds its configured bound")]
    ResponseTooLarge,
    #[error("local stdio executable is not a regular non-symlink file")]
    UnsafeExecutable,
    #[error("local stdio executable digest does not match the reviewed binary")]
    ExecutableDigestMismatch,
    #[error("local stdio executable digest could not be represented")]
    Digest,
    #[error("local stdio process did not start within its bound")]
    StartupTimeout,
    #[error("local stdio call timed out")]
    CallTimeout,
    #[error("local stdio restart budget is exhausted")]
    RestartBudgetExhausted,
    #[error("local stdio child closed its output")]
    Closed,
    #[error("local stdio child pipe was unavailable")]
    MissingPipe,
    #[error("local stdio child could not be spawned: {0}")]
    Spawn(io::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

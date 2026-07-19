use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use commonkit_platform::AppPaths;
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_updater::UpdaterExt;
use thiserror::Error;

#[derive(Debug, Clone)]
struct ServiceClient {
    discovery: PathBuf,
    token: PathBuf,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeBinaryLayout {
    cli: PathBuf,
    daemon: PathBuf,
    target_helper: PathBuf,
}

impl RuntimeBinaryLayout {
    fn beside(desktop: &Path) -> Self {
        let directory = desktop.parent().unwrap_or_else(|| Path::new("."));
        let extension = std::env::consts::EXE_SUFFIX;
        Self {
            cli: directory.join(format!("commonkit{extension}")),
            daemon: directory.join(format!("commonkitd{extension}")),
            target_helper: directory.join(format!("commonkit-target-helper{extension}")),
        }
    }

    fn discover() -> Result<Self, DesktopError> {
        let installed = Self::beside(&std::env::current_exe()?);
        if installed.validate().is_ok() {
            return Ok(installed);
        }
        #[cfg(debug_assertions)]
        {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug");
            let development = Self::beside(
                &workspace.join(format!("commonkit-desktop{}", std::env::consts::EXE_SUFFIX)),
            );
            development.validate()?;
            return Ok(development);
        }
        #[cfg(not(debug_assertions))]
        Err(DesktopError::RuntimeBinaryMissing)
    }

    fn validate(&self) -> Result<(), DesktopError> {
        for path in [&self.cli, &self.daemon, &self.target_helper] {
            let metadata =
                std::fs::symlink_metadata(path).map_err(|_| DesktopError::RuntimeBinaryMissing)?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(DesktopError::RuntimeBinaryMissing);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ServiceSupervisor {
    child: Mutex<Option<Child>>,
}

impl ServiceSupervisor {
    fn ensure_started(client: &ServiceClient) -> Result<Self, DesktopError> {
        if service_is_authenticated(client) {
            return Ok(Self {
                child: Mutex::new(None),
            });
        }
        let runtime = RuntimeBinaryLayout::discover()?;
        let child = Command::new(runtime.daemon)
            .args(["--port", "0", "--relay-port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DesktopError::ServiceLaunchFailed)?;
        let supervisor = Self {
            child: Mutex::new(Some(child)),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if service_is_authenticated(client) {
                return Ok(supervisor);
            }
            if supervisor.child_exited()? {
                return Err(DesktopError::ServiceLaunchFailed);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        supervisor.stop();
        Err(DesktopError::ServiceLaunchFailed)
    }

    fn child_exited(&self) -> Result<bool, DesktopError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| DesktopError::ServiceLaunchFailed)?;
        Ok(match child.as_mut() {
            Some(child) => child.try_wait()?.is_some(),
            None => false,
        })
    }

    /// Replace the daemon only when this desktop instance owns its process.
    ///
    /// An empty slot means Desktop attached to an independently managed daemon;
    /// callers must never terminate or replace that process.
    fn replace_owned_child(
        &self,
        spawn: impl FnOnce() -> Result<Child, DesktopError>,
    ) -> Result<bool, DesktopError> {
        let mut slot = self
            .child
            .lock()
            .map_err(|_| DesktopError::ServiceLaunchFailed)?;
        let Some(mut previous) = slot.take() else {
            return Ok(false);
        };
        let _ = previous.kill();
        let _ = previous.wait();
        let replacement = spawn()?;
        *slot = Some(replacement);
        Ok(true)
    }

    fn stop(&self) {
        let Ok(mut slot) = self.child.lock() else {
            return;
        };
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ServiceSupervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn service_is_authenticated(client: &ServiceClient) -> bool {
    tauri::async_runtime::block_on(client.status()).is_ok()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceStatus {
    api_version: String,
    contract_version: String,
    schema_version: u32,
    runtime_version: String,
    state: String,
    active_target: Option<String>,
    active_loadout: Option<String>,
    last_drift_check_unix_ms: Option<u128>,
    last_drift_error_code: Option<String>,
}

impl ServiceStatus {
    fn offline() -> Self {
        Self {
            api_version: "v1".into(),
            contract_version: "1.0".into(),
            schema_version: 1,
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            state: "offline".into(),
            active_target: None,
            active_loadout: None,
            last_drift_check_unix_ms: None,
            last_drift_error_code: Some("service_unavailable".into()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityState {
    id: &'static str,
    ready: bool,
    detail: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopSnapshot {
    status: ServiceStatus,
    capabilities: Vec<CapabilityState>,
    last_event_id: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementSnapshot {
    plans: serde_json::Value,
    credentials: serde_json::Value,
    snapshots: serde_json::Value,
    relay: serde_json::Value,
    schedule: serde_json::Value,
    diagnostics: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraySummary {
    health: String,
    loadout: String,
    relay: String,
    snapshots: String,
}

#[derive(Clone)]
struct TrayItems {
    health: MenuItem<tauri::Wry>,
    loadout: MenuItem<tauri::Wry>,
    relay: MenuItem<tauri::Wry>,
    snapshots: MenuItem<tauri::Wry>,
}

impl TraySummary {
    fn from_observed(
        status: &ServiceStatus,
        relay: Option<&serde_json::Value>,
        snapshots: Option<&serde_json::Value>,
    ) -> Self {
        let target = status.active_target.as_deref().unwrap_or("no target");
        let loadout = status.active_loadout.as_deref().unwrap_or("not selected");
        let relay = relay
            .and_then(|value| value.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unavailable");
        let snapshots = snapshots
            .and_then(|value| value.get("snapshots"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::len);
        Self {
            health: format!("Health: {}", status.state),
            loadout: format!("Loadout: {loadout} · {target}"),
            relay: format!("Relay: {relay}"),
            snapshots: snapshots
                .map(|count| format!("Snapshots: {count} available"))
                .unwrap_or_else(|| "Snapshots: unavailable".into()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSummary {
    current_version: String,
    version: String,
    notes: Option<String>,
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OnboardingRequest {
    mode: String,
    repository: String,
    kit_directory: PathBuf,
    loadout: String,
    target: String,
    target_root: PathBuf,
    provider: String,
    provider_executable: Option<PathBuf>,
    provider_version: Option<String>,
    apm_manifest: Option<PathBuf>,
    apm_lockfile: Option<PathBuf>,
    apm_policy: Option<PathBuf>,
    chezmoi_source: Option<PathBuf>,
    chezmoi_config: Option<PathBuf>,
    publish_registration: bool,
}

fn onboarding_arguments(request: &OnboardingRequest) -> Result<Vec<String>, DesktopError> {
    if !matches!(request.mode.as_str(), "create" | "connect")
        || request.repository.split('/').count() != 2
        || !request.kit_directory.is_absolute()
        || !request.target_root.is_absolute()
    {
        return Err(DesktopError::InvalidInput);
    }
    validate_id(&request.loadout)?;
    validate_id(&request.target)?;
    let mut args = vec![
        "init".into(),
        request.mode.clone(),
        "--repository".into(),
        request.repository.clone(),
        "--kit-directory".into(),
        path_string(&request.kit_directory)?,
        "--loadout".into(),
        request.loadout.clone(),
        "--target".into(),
        request.target.clone(),
        "--target-root".into(),
        path_string(&request.target_root)?,
        "--provider".into(),
        request.provider.clone(),
    ];
    match request.provider.as_str() {
        "native" if request.provider_executable.is_none() && request.provider_version.is_none() => {
        }
        "apm" => {
            require_onboarding_version(request, "0.25.0")?;
            args.extend(["--provider-version".into(), "0.25.0".into()]);
            push_provider_path(
                &mut args,
                "--provider-executable",
                request.provider_executable.as_ref(),
                true,
            )?;
            push_provider_path(
                &mut args,
                "--apm-manifest",
                request.apm_manifest.as_ref(),
                false,
            )?;
            push_provider_path(
                &mut args,
                "--apm-lockfile",
                request.apm_lockfile.as_ref(),
                false,
            )?;
            push_provider_path(
                &mut args,
                "--apm-policy",
                request.apm_policy.as_ref(),
                false,
            )?;
        }
        "chezmoi" => {
            require_onboarding_version(request, "2.70.4")?;
            args.extend(["--provider-version".into(), "2.70.4".into()]);
            push_provider_path(
                &mut args,
                "--provider-executable",
                request.provider_executable.as_ref(),
                true,
            )?;
            push_provider_path(
                &mut args,
                "--chezmoi-source",
                request.chezmoi_source.as_ref(),
                false,
            )?;
            push_provider_path(
                &mut args,
                "--chezmoi-config",
                request.chezmoi_config.as_ref(),
                false,
            )?;
        }
        _ => return Err(DesktopError::InvalidInput),
    }
    Ok(args)
}

fn require_onboarding_version(
    request: &OnboardingRequest,
    expected: &str,
) -> Result<(), DesktopError> {
    if request.provider_version.as_deref() != Some(expected) {
        return Err(DesktopError::InvalidInput);
    }
    Ok(())
}

fn push_provider_path(
    args: &mut Vec<String>,
    flag: &str,
    value: Option<&PathBuf>,
    absolute: bool,
) -> Result<(), DesktopError> {
    let value = value.ok_or(DesktopError::InvalidInput)?;
    if absolute != value.is_absolute()
        || (!absolute
            && value
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)))
    {
        return Err(DesktopError::InvalidInput);
    }
    args.extend([flag.into(), path_string(value)?]);
    Ok(())
}

fn path_string(path: &std::path::Path) -> Result<String, DesktopError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(DesktopError::InvalidInput)
}

#[tauri::command]
fn onboarding_initialize(
    request: OnboardingRequest,
    client: tauri::State<'_, ServiceClient>,
) -> Result<serde_json::Value, DesktopError> {
    let _ = onboarding_arguments(&request)?;
    use commonkit_cli::onboarding::{
        InitMode, InitRequest, ProcessRunner, ProviderSelection, initialize,
    };
    // Fail before cloning, committing, or publishing anything when the
    // attached daemon cannot atomically adopt the resulting domains.
    tauri::async_runtime::block_on(client.json(reqwest::Method::GET, "/domains/reload", None))?;
    let paths = AppPaths::discover().map_err(|_| DesktopError::OnboardingFailed)?;
    let provider = match request.provider.as_str() {
        "native" => ProviderSelection::Native,
        "apm" => ProviderSelection::Apm {
            executable: request
                .provider_executable
                .ok_or(DesktopError::InvalidInput)?,
            manifest: request.apm_manifest.ok_or(DesktopError::InvalidInput)?,
            lockfile: request.apm_lockfile.ok_or(DesktopError::InvalidInput)?,
            policy: request.apm_policy.ok_or(DesktopError::InvalidInput)?,
        },
        "chezmoi" => ProviderSelection::Chezmoi {
            executable: request
                .provider_executable
                .ok_or(DesktopError::InvalidInput)?,
            source: request.chezmoi_source.ok_or(DesktopError::InvalidInput)?,
            config: request.chezmoi_config.ok_or(DesktopError::InvalidInput)?,
        },
        _ => return Err(DesktopError::InvalidInput),
    };
    let headless_config = paths.config.join("headless.json");
    let previous_config = match std::fs::read(&headless_config) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let result = initialize(
        &InitRequest {
            mode: if request.mode == "create" {
                InitMode::Create
            } else {
                InitMode::Connect
            },
            repository: request.repository,
            kit_directory: request.kit_directory,
            loadout: request.loadout,
            target: request.target,
            target_root: request.target_root,
            config_directory: paths.config,
            state_directory: paths.state,
            provider,
            publish_registration: request.publish_registration,
        },
        &ProcessRunner::from_path(),
    )
    .map_err(|_| DesktopError::OnboardingFailed)?;
    if tauri::async_runtime::block_on(client.json(
        reqwest::Method::POST,
        "/domains/reload",
        Some(serde_json::json!({"confirmed": true})),
    ))
    .is_err()
    {
        restore_headless_config(&headless_config, previous_config.as_deref())?;
        // The old runtime remains active if validation failed. Re-adopt the
        // restored file so durable and in-memory state agree before returning.
        let _ = tauri::async_runtime::block_on(client.json(
            reqwest::Method::POST,
            "/domains/reload",
            Some(serde_json::json!({"confirmed": true})),
        ));
        return Err(DesktopError::ServiceReloadFailed);
    }
    serde_json::to_value(result).map_err(Into::into)
}

fn restore_headless_config(path: &Path, previous: Option<&[u8]>) -> Result<(), DesktopError> {
    match previous {
        Some(bytes) => {
            let temporary = path.with_extension(format!("rollback-{}.tmp", std::process::id()));
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            #[cfg(windows)]
            std::fs::remove_file(path)?;
            std::fs::rename(temporary, path)?;
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

impl ServiceClient {
    fn discover() -> Result<Self, DesktopError> {
        let paths = AppPaths::discover().map_err(|_| DesktopError::ServiceUnavailable)?;
        Ok(Self {
            discovery: paths.state.join("daemon.json"),
            token: paths.config.join("control.token"),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(4))
                .build()?,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, DesktopError> {
        let discovery: Discovery =
            serde_json::from_slice(&tokio::fs::read(&self.discovery).await?)?;
        let token = String::from_utf8(tokio::fs::read(&self.token).await?)
            .map_err(|_| DesktopError::InvalidControlState)?;
        if token.len() < 32 || token.chars().any(char::is_whitespace) {
            return Err(DesktopError::InvalidControlState);
        }
        Ok(self
            .http
            .request(
                method,
                format!("http://127.0.0.1:{}/control/v1{path}", discovery.port),
            )
            .bearer_auth(token))
    }

    async fn status(&self) -> Result<ServiceStatus, DesktopError> {
        Ok(self
            .request(reqwest::Method::GET, "/status")
            .await?
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, DesktopError> {
        let request = self.request(method, path).await?;
        let request = match body {
            Some(body) => request.json(&body),
            None => request,
        };
        Ok(request.send().await?.error_for_status()?.json().await?)
    }

    async fn apply(
        &self,
        target_id: &str,
        plan_id: &str,
        confirmation_id: &str,
    ) -> Result<serde_json::Value, DesktopError> {
        validate_id(confirmation_id)?;
        let route = target_apply_route(target_id, plan_id)?;
        let key = format!("desktop-{}", &plan_id[7..23]);
        Ok(self
            .request(reqwest::Method::POST, &route)
            .await?
            .header("idempotency-key", key)
            .json(&serde_json::json!({"confirmed": true, "confirmationId": confirmation_id}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

fn target_convergence_route(target_id: &str, action: &str) -> Result<String, DesktopError> {
    validate_id(target_id)?;
    if !matches!(action, "sync/plan" | "verify") {
        return Err(DesktopError::InvalidInput);
    }
    Ok(format!("/targets/{target_id}/{action}"))
}

fn target_apply_route(target_id: &str, plan_id: &str) -> Result<String, DesktopError> {
    validate_digest(plan_id)?;
    Ok(format!(
        "{}/plans/{plan_id}/apply",
        target_convergence_route(target_id, "verify")?.trim_end_matches("/verify")
    ))
}

fn management_routes() -> [(&'static str, &'static str); 6] {
    [
        ("plans", "/compose"),
        ("credentials", "/credentials/readiness"),
        ("snapshots", "/snapshots"),
        ("relay", "/relay"),
        ("schedule", "/schedule"),
        ("diagnostics", "/diagnostics"),
    ]
}

fn operator_routes() -> [(&'static str, &'static str); 12] {
    [
        ("plan", "/targets/{target}/sync/plan"),
        ("verify", "/targets/{target}/verify"),
        ("apply", "/targets/{target}/plans/{plan}/apply"),
        ("snapshot_create", "/snapshots"),
        ("snapshot_restore", "/snapshots/restore"),
        ("snapshot_promote", "/snapshots/promote"),
        ("relay_reconcile", "/relay/reconcile"),
        ("relay_restart", "/relay/restart"),
        ("schedule", "/schedule"),
        ("credential_apply", "/credentials/apply"),
        ("credential_verify", "/credentials/verify"),
        ("diagnostics", "/diagnostics"),
    ]
}

fn unavailable(error: DesktopError) -> serde_json::Value {
    serde_json::json!({ "error": error.to_string() })
}

fn validate_digest(value: &str) -> Result<(), DesktopError> {
    if value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(DesktopError::InvalidInput)
    }
}

fn validate_id(value: &str) -> Result<(), DesktopError> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(DesktopError::InvalidInput)
    }
}

#[derive(Debug, Error)]
enum DesktopError {
    #[error("CommonKit service is unavailable")]
    ServiceUnavailable,
    #[error("the installed CommonKit runtime binaries are missing or unsafe")]
    RuntimeBinaryMissing,
    #[error("the bundled CommonKit service could not be started")]
    ServiceLaunchFailed,
    #[error(
        "CommonKit saved the new configuration, but the independently managed service must be restarted by its service manager before continuing"
    )]
    ExternalServiceReloadRequired,
    #[error(
        "CommonKit saved the new configuration, but its managed service could not reload it; reopen CommonKit and retry"
    )]
    ServiceReloadFailed,
    #[error("local control state is invalid")]
    InvalidControlState,
    #[error("input is invalid")]
    InvalidInput,
    #[error(
        "CommonKit onboarding failed; review the selected provider inputs and local CLI diagnostics"
    )]
    OnboardingFailed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

impl Serialize for DesktopError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[tauri::command]
async fn desktop_snapshot(
    client: tauri::State<'_, ServiceClient>,
) -> Result<DesktopSnapshot, DesktopError> {
    let status = client
        .status()
        .await
        .unwrap_or_else(|_| ServiceStatus::offline());
    let online = status.state != "offline";
    Ok(DesktopSnapshot {
        status,
        last_event_id: None,
        capabilities: vec![
            CapabilityState {
                id: "status",
                ready: online,
                detail: "Local target status and drift",
            },
            CapabilityState {
                id: "plans",
                ready: online,
                detail: "Content-bound review and confirmation",
            },
            CapabilityState {
                id: "credentials",
                ready: false,
                detail: "Awaiting service capability",
            },
            CapabilityState {
                id: "snapshots",
                ready: false,
                detail: "Awaiting service capability",
            },
            CapabilityState {
                id: "relay",
                ready: false,
                detail: "Awaiting service capability",
            },
            CapabilityState {
                id: "schedule",
                ready: false,
                detail: "Awaiting service capability",
            },
            CapabilityState {
                id: "diagnostics",
                ready: online,
                detail: "Redacted diagnostic export",
            },
        ],
    })
}

#[tauri::command]
async fn desktop_management_snapshot(
    client: tauri::State<'_, ServiceClient>,
) -> Result<ManagementSnapshot, DesktopError> {
    let plans = client
        .json(reqwest::Method::GET, "/compose", None)
        .await
        .unwrap_or_else(unavailable);
    let credentials = client
        .json(
            reqwest::Method::POST,
            "/credentials/readiness",
            Some(serde_json::json!({"references": []})),
        )
        .await
        .unwrap_or_else(unavailable);
    let snapshots = client
        .json(reqwest::Method::GET, "/snapshots", None)
        .await
        .unwrap_or_else(unavailable);
    let relay = client
        .json(reqwest::Method::GET, "/relay", None)
        .await
        .unwrap_or_else(unavailable);
    let schedule = client
        .json(reqwest::Method::GET, "/schedule", None)
        .await
        .unwrap_or_else(unavailable);
    let diagnostics = client
        .json(reqwest::Method::GET, "/diagnostics", None)
        .await
        .unwrap_or_else(unavailable);
    Ok(ManagementSnapshot {
        plans,
        credentials,
        snapshots,
        relay,
        schedule,
        diagnostics,
    })
}

#[tauri::command]
async fn apply_plan(
    client: tauri::State<'_, ServiceClient>,
    target_id: String,
    plan_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    client.apply(&target_id, &plan_id, &confirmation_id).await
}

#[tauri::command]
async fn plan_sync(
    client: tauri::State<'_, ServiceClient>,
    target_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    let path = target_convergence_route(&target_id, "sync/plan")?;
    post_confirmed(&client, &path, serde_json::json!({}), &confirmation_id).await
}

fn confirmed_body(
    mut body: serde_json::Value,
    confirmation_id: &str,
) -> Result<serde_json::Value, DesktopError> {
    validate_id(confirmation_id)?;
    let object = body.as_object_mut().ok_or(DesktopError::InvalidInput)?;
    object.insert("confirmed".into(), true.into());
    object.insert("confirmationId".into(), confirmation_id.into());
    Ok(body)
}

async fn post_confirmed(
    client: &ServiceClient,
    path: &str,
    body: serde_json::Value,
    confirmation_id: &str,
) -> Result<serde_json::Value, DesktopError> {
    client
        .json(
            reqwest::Method::POST,
            path,
            Some(confirmed_body(body, confirmation_id)?),
        )
        .await
}

#[tauri::command]
async fn desktop_verify(
    client: tauri::State<'_, ServiceClient>,
    target_id: String,
) -> Result<serde_json::Value, DesktopError> {
    let path = target_convergence_route(&target_id, "verify")?;
    client
        .json(
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({"targetId": target_id, "pointer": null})),
        )
        .await
}

#[tauri::command]
async fn targets_list(
    client: tauri::State<'_, ServiceClient>,
) -> Result<serde_json::Value, DesktopError> {
    client.json(reqwest::Method::GET, "/targets", None).await
}

#[tauri::command]
async fn targets_select(
    client: tauri::State<'_, ServiceClient>,
    targets: Vec<String>,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    for target in &targets {
        validate_id(target)?;
    }
    post_confirmed(
        &client,
        "/targets/select",
        serde_json::json!({"targets": targets}),
        &confirmation_id,
    )
    .await?;
    client.json(reqwest::Method::GET, "/targets", None).await
}

#[tauri::command]
async fn snapshot_create(
    client: tauri::State<'_, ServiceClient>,
    database_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_id(&database_id)?;
    post_confirmed(
        &client,
        "/snapshots",
        serde_json::json!({"databaseId": database_id}),
        &confirmation_id,
    )
    .await
}

#[tauri::command]
async fn snapshot_restore(
    client: tauri::State<'_, ServiceClient>,
    snapshot_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_id(&snapshot_id)?;
    post_confirmed(
        &client,
        "/snapshots/restore",
        serde_json::json!({"snapshotId": snapshot_id}),
        &confirmation_id,
    )
    .await
}

#[tauri::command]
async fn snapshot_promote(
    client: tauri::State<'_, ServiceClient>,
    database_id: String,
    target_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_id(&database_id)?;
    validate_id(&target_id)?;
    post_confirmed(
        &client,
        "/snapshots/promote",
        serde_json::json!({"databaseId": database_id, "targetId": target_id}),
        &confirmation_id,
    )
    .await
}

#[tauri::command]
async fn relay_reconcile(
    client: tauri::State<'_, ServiceClient>,
    request: serde_json::Value,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    post_confirmed(&client, "/relay/reconcile", request, &confirmation_id).await
}

#[tauri::command]
async fn relay_restart(
    client: tauri::State<'_, ServiceClient>,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    post_confirmed(
        &client,
        "/relay/restart",
        serde_json::json!({}),
        &confirmation_id,
    )
    .await
}

#[tauri::command]
async fn schedule_configure(
    client: tauri::State<'_, ServiceClient>,
    enabled: bool,
    interval_seconds: u64,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    if interval_seconds == 0 {
        return Err(DesktopError::InvalidInput);
    }
    let body = if enabled {
        serde_json::json!({"enabled": true, "intervalSeconds": interval_seconds})
    } else {
        serde_json::json!({"enabled": false})
    };
    post_confirmed(&client, "/schedule", body, &confirmation_id).await
}

fn validate_ids(values: &[String]) -> Result<(), DesktopError> {
    if values.is_empty() {
        return Err(DesktopError::InvalidInput);
    }
    values.iter().try_for_each(|value| validate_id(value))
}

#[tauri::command]
async fn credential_readiness(
    client: tauri::State<'_, ServiceClient>,
    references: Vec<String>,
) -> Result<serde_json::Value, DesktopError> {
    client
        .json(
            reqwest::Method::POST,
            "/credentials/readiness",
            Some(serde_json::json!({"references": references})),
        )
        .await
}

#[tauri::command]
async fn credential_apply(
    client: tauri::State<'_, ServiceClient>,
    destination_ids: Vec<String>,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_ids(&destination_ids)?;
    post_confirmed(
        &client,
        "/credentials/apply",
        serde_json::json!({"destinationIds": destination_ids}),
        &confirmation_id,
    )
    .await
}

#[tauri::command]
async fn credential_verify(
    client: tauri::State<'_, ServiceClient>,
    destination_ids: Vec<String>,
) -> Result<serde_json::Value, DesktopError> {
    validate_ids(&destination_ids)?;
    client
        .json(
            reqwest::Method::POST,
            "/credentials/verify",
            Some(serde_json::json!({"destinationIds": destination_ids})),
        )
        .await
}

#[tauri::command]
async fn diagnostics_export(
    client: tauri::State<'_, ServiceClient>,
) -> Result<serde_json::Value, DesktopError> {
    client
        .json(reqwest::Method::GET, "/diagnostics", None)
        .await
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| error.to_string())?;
    manager.is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateSummary>, String> {
    let Some(update) = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    Ok(Some(UpdateSummary {
        current_version: update.current_version,
        version: update.version,
        notes: update.body,
        published_at: update.date.map(|date| date.to_string()),
    }))
}

fn authorize_update_install(
    confirmed: bool,
    expected: &str,
    available: &str,
) -> Result<(), String> {
    if !confirmed {
        return Err("explicit update confirmation is required".into());
    }
    validate_id(expected).map_err(|error| error.to_string())?;
    if expected != available {
        return Err("the available update changed; review and confirm it again".into());
    }
    Ok(())
}

fn write_update_report(path: &Path, payload: &serde_json::Value) -> std::io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(payload).map_err(std::io::Error::other)?;
    let mut file = std::fs::File::create(&temporary)?;
    use std::io::Write as _;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn install_after_durable_handoff<T, Handoff, Install>(
    verified_bytes: T,
    persist_handoff: Handoff,
    install: Install,
) -> Result<(), String>
where
    Handoff: FnOnce() -> Result<(), String>,
    Install: FnOnce(T) -> Result<(), String>,
{
    persist_handoff()?;
    install(verified_bytes)
}

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    expected_version: String,
    confirmed: bool,
) -> Result<(), String> {
    let Some(update) = app
        .updater()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("the confirmed update is no longer available".into());
    };
    authorize_update_install(confirmed, &expected_version, &update.version)?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| error.to_string())
}

async fn run_automated_update_lifecycle(app: tauri::AppHandle, report: PathBuf, expected: String) {
    let result = async {
        #[cfg(target_os = "windows")]
        let updater = {
            let marker_app = app.clone();
            app.updater_builder()
                .on_before_exit(move || {
                    marker_app.cleanup_before_exit();
                })
                .build()
                .map_err(|error| error.to_string())?
        };
        #[cfg(not(target_os = "windows"))]
        let updater = app.updater().map_err(|error| error.to_string())?;
        let update = updater
            .check()
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the expected published update is unavailable".to_owned())?;
        authorize_update_install(true, &expected, &update.version)?;
        #[cfg(target_os = "windows")]
        {
            let bytes = update
                .download(|_, _| {}, || {})
                .await
                .map_err(|error| error.to_string())?;
            let marker = serde_json::json!({
                "schemaVersion": 1,
                "previousVersion": env!("CARGO_PKG_VERSION"),
                "expectedVersion": expected,
                "updaterExitPrepared": true,
            });
            install_after_durable_handoff(
                bytes,
                || write_update_report(&report, &marker).map_err(|error| error.to_string()),
                |bytes| update.install(bytes).map_err(|error| error.to_string()),
            )?;
        }
        #[cfg(not(target_os = "windows"))]
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(serde_json::json!({
            "schemaVersion": 1,
            "previousVersion": env!("CARGO_PKG_VERSION"),
            "updatedByTauri": true,
        }))
    }
    .await;
    let (exit_code, payload) = match result {
        Ok(payload) => (0, payload),
        Err(error) => (
            70,
            serde_json::json!({
                "schemaVersion": 1,
                "previousVersion": env!("CARGO_PKG_VERSION"),
                "updatedByTauri": false,
                "error": error,
            }),
        ),
    };
    if write_update_report(&report, &payload).is_err() {
        app.exit(71);
        return;
    }
    app.exit(exit_code);
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn show_route(app: tauri::AppHandle, route: &str) -> Result<(), String> {
    if !matches!(route, "status" | "plans" | "relay" | "snapshots") {
        return Err("invalid desktop route".into());
    }
    show_main_window(app.clone())?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    window
        .eval(format!("window.location.hash = '#{route}'"))
        .map_err(|error| error.to_string())
}

async fn refresh_tray(app: tauri::AppHandle) {
    let client = app.state::<ServiceClient>().inner().clone();
    let status = client
        .status()
        .await
        .unwrap_or_else(|_| ServiceStatus::offline());
    let relay = client.json(reqwest::Method::GET, "/relay", None).await.ok();
    let snapshots = client
        .json(reqwest::Method::GET, "/snapshots", None)
        .await
        .ok();
    let summary = TraySummary::from_observed(&status, relay.as_ref(), snapshots.as_ref());
    let items = app.state::<TrayItems>();
    let _ = items.health.set_text(&summary.health);
    let _ = items.loadout.set_text(&summary.loadout);
    let _ = items.relay.set_text(&summary.relay);
    let _ = items.snapshots.set_text(&summary.snapshots);

    if let Some(report) = std::env::var_os("COMMONKIT_DESKTOP_SMOKE_REPORT") {
        let runtime = RuntimeBinaryLayout::discover();
        let payload = serde_json::json!({
            "schemaVersion": 1,
            "desktopVersion": env!("CARGO_PKG_VERSION"),
            "desktopExecutable": std::env::current_exe().ok(),
            "serviceState": status.state,
            "runtimeVersion": status.runtime_version,
            "bundledCli": runtime.as_ref().ok().map(|value| &value.cli),
            "bundledDaemon": runtime.as_ref().ok().map(|value| &value.daemon),
            "bundledTargetHelper": runtime.as_ref().ok().map(|value| &value.target_helper),
            "tray": summary,
        });
        if runtime.is_ok()
            && status.state != "offline"
            && std::fs::write(
                &report,
                serde_json::to_vec_pretty(&payload).unwrap_or_default(),
            )
            .is_ok()
        {
            app.exit(0);
        } else {
            app.exit(70);
        }
    }
}

pub fn run() {
    let client = ServiceClient::discover().unwrap_or_else(|_| ServiceClient {
        discovery: PathBuf::new(),
        token: PathBuf::new(),
        http: reqwest::Client::new(),
    });
    tauri::Builder::default()
        .manage(client.clone())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show_main_window(app.clone());
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--minimized"])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            desktop_snapshot,
            onboarding_initialize,
            desktop_management_snapshot,
            apply_plan,
            plan_sync,
            desktop_verify,
            targets_list,
            targets_select,
            snapshot_create,
            snapshot_restore,
            snapshot_promote,
            relay_reconcile,
            relay_restart,
            schedule_configure,
            credential_readiness,
            credential_apply,
            credential_verify,
            diagnostics_export,
            set_autostart,
            check_for_update,
            install_update,
            show_main_window
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let supervisor = ServiceSupervisor::ensure_started(&client)?;
            app.manage(supervisor);
            let health = MenuItem::with_id(app, "health", "Health: starting", false, None::<&str>)?;
            let loadout =
                MenuItem::with_id(app, "loadout", "Loadout: loading", false, None::<&str>)?;
            let relay =
                MenuItem::with_id(app, "relay-status", "Relay: loading", false, None::<&str>)?;
            let snapshots = MenuItem::with_id(
                app,
                "snapshot-status",
                "Snapshots: loading",
                false,
                None::<&str>,
            )?;
            let refresh = MenuItem::with_id(app, "refresh", "Refresh health", true, None::<&str>)?;
            let review = MenuItem::with_id(app, "review", "Review plan…", true, None::<&str>)?;
            let manage_relay =
                MenuItem::with_id(app, "manage-relay", "Manage relay…", true, None::<&str>)?;
            let manage_snapshots = MenuItem::with_id(
                app,
                "manage-snapshots",
                "Manage snapshots…",
                true,
                None::<&str>,
            )?;
            let open = MenuItem::with_id(app, "open", "Open CommonKit", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            app.manage(TrayItems {
                health: health.clone(),
                loadout: loadout.clone(),
                relay: relay.clone(),
                snapshots: snapshots.clone(),
            });
            let menu = Menu::with_items(
                app,
                &[
                    &health,
                    &loadout,
                    &relay,
                    &snapshots,
                    &refresh,
                    &review,
                    &manage_relay,
                    &manage_snapshots,
                    &open,
                    &quit,
                ],
            )?;
            TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("CommonKit")
                .icon(
                    app.default_window_icon()
                        .ok_or("desktop icon unavailable")?
                        .clone(),
                )
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        let _ = show_main_window(app.clone());
                    }
                    "refresh" => {
                        tauri::async_runtime::spawn(refresh_tray(app.clone()));
                    }
                    "review" => {
                        let _ = show_route(app.clone(), "plans");
                    }
                    "manage-relay" => {
                        let _ = show_route(app.clone(), "relay");
                    }
                    "manage-snapshots" => {
                        let _ = show_route(app.clone(), "snapshots");
                    }
                    "quit" => {
                        app.state::<ServiceSupervisor>().stop();
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            let app_handle = app.handle().clone();
            if let (Some(report), Ok(expected)) = (
                std::env::var_os("COMMONKIT_DESKTOP_UPDATE_REPORT"),
                std::env::var("COMMONKIT_DESKTOP_UPDATE_EXPECTED_VERSION"),
            ) {
                if expected == env!("CARGO_PKG_VERSION") {
                    // The Windows installer may automatically relaunch the replacement while
                    // inheriting the old process environment. Leave the durable launch marker
                    // untouched and exit so CI can start a clean, independent smoke process.
                    app_handle.exit(0);
                } else {
                    tauri::async_runtime::spawn(run_automated_update_lifecycle(
                        app_handle.clone(),
                        report.into(),
                        expected,
                    ));
                }
            }
            tauri::async_runtime::spawn(async move {
                loop {
                    refresh_tray(app_handle.clone()).await;
                    tokio::time::sleep(Duration::from_secs(15)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run CommonKit desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn onboarding_replaces_only_a_desktop_owned_daemon_process() {
        let first = Command::new("sleep").arg("60").spawn().unwrap();
        let first_id = first.id();
        let supervisor = ServiceSupervisor {
            child: Mutex::new(Some(first)),
        };

        let replaced = supervisor
            .replace_owned_child(|| Command::new("sleep").arg("60").spawn().map_err(Into::into))
            .unwrap();

        assert!(replaced);
        let second_id = supervisor
            .child
            .lock()
            .unwrap()
            .as_ref()
            .expect("replacement process")
            .id();
        assert_ne!(first_id, second_id);

        supervisor.stop();
        let attached = ServiceSupervisor {
            child: Mutex::new(None),
        };
        assert!(
            !attached
                .replace_owned_child(|| panic!("an externally managed daemon must not be replaced"))
                .unwrap()
        );
    }

    #[test]
    fn packaged_runtime_binaries_are_resolved_only_beside_the_installed_desktop() {
        let executable = if cfg!(windows) {
            "commonkit-desktop.exe"
        } else {
            "commonkit-desktop"
        };
        let root = Path::new("/Applications/CommonKit.app/Contents/MacOS");
        let layout = RuntimeBinaryLayout::beside(&root.join(executable));
        assert_eq!(
            layout.cli,
            root.join(if cfg!(windows) {
                "commonkit.exe"
            } else {
                "commonkit"
            })
        );
        assert_eq!(
            layout.daemon,
            root.join(if cfg!(windows) {
                "commonkitd.exe"
            } else {
                "commonkitd"
            })
        );
        assert_eq!(
            layout.target_helper,
            root.join(if cfg!(windows) {
                "commonkit-target-helper.exe"
            } else {
                "commonkit-target-helper"
            })
        );
    }

    #[test]
    fn tray_summary_is_derived_from_observed_daemon_state() {
        let status = ServiceStatus {
            state: "degraded".into(),
            active_target: Some("macbook".into()),
            active_loadout: Some("personal".into()),
            ..ServiceStatus::offline()
        };
        let summary = TraySummary::from_observed(
            &status,
            Some(&serde_json::json!({"state":"healthy"})),
            Some(&serde_json::json!({"snapshots":[{}, {}]})),
        );
        assert_eq!(summary.health, "Health: degraded");
        assert_eq!(summary.loadout, "Loadout: personal · macbook");
        assert_eq!(summary.relay, "Relay: healthy");
        assert_eq!(summary.snapshots, "Snapshots: 2 available");
    }
    #[test]
    fn management_snapshot_uses_only_fixed_read_only_service_routes() {
        assert_eq!(
            management_routes(),
            [
                ("plans", "/compose"),
                ("credentials", "/credentials/readiness"),
                ("snapshots", "/snapshots"),
                ("relay", "/relay"),
                ("schedule", "/schedule"),
                ("diagnostics", "/diagnostics"),
            ]
        );
    }
    #[test]
    fn operator_actions_are_bound_to_fixed_service_routes_and_methods() {
        assert_eq!(
            operator_routes(),
            [
                ("plan", "/targets/{target}/sync/plan"),
                ("verify", "/targets/{target}/verify"),
                ("apply", "/targets/{target}/plans/{plan}/apply"),
                ("snapshot_create", "/snapshots"),
                ("snapshot_restore", "/snapshots/restore"),
                ("snapshot_promote", "/snapshots/promote"),
                ("relay_reconcile", "/relay/reconcile"),
                ("relay_restart", "/relay/restart"),
                ("schedule", "/schedule"),
                ("credential_apply", "/credentials/apply"),
                ("credential_verify", "/credentials/verify"),
                ("diagnostics", "/diagnostics"),
            ]
        );
    }
    #[test]
    fn validates_plan_bound_command_inputs() {
        assert!(validate_digest(&format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(validate_digest("../../control.token").is_err());
        assert!(validate_id("desktop-confirmation-1").is_ok());
        assert!(validate_id("bad/value").is_err());
    }
    #[test]
    fn convergence_routes_are_scoped_to_the_selected_target() {
        assert_eq!(
            target_convergence_route("workstation-a", "sync/plan").unwrap(),
            "/targets/workstation-a/sync/plan"
        );
        assert_eq!(
            target_convergence_route("workstation-a", "verify").unwrap(),
            "/targets/workstation-a/verify"
        );
        assert_eq!(
            target_apply_route("workstation-a", &format!("sha256:{}", "a".repeat(64))).unwrap(),
            format!(
                "/targets/workstation-a/plans/sha256:{}/apply",
                "a".repeat(64)
            )
        );
        assert!(target_convergence_route("../other", "verify").is_err());
        assert!(target_apply_route("workstation-a", "stale-plan").is_err());
    }
    #[test]
    fn update_install_requires_consent_bound_to_the_observed_version() {
        assert!(authorize_update_install(false, "0.2.0", "0.2.0").is_err());
        assert!(authorize_update_install(true, "0.2.0", "0.3.0").is_err());
        assert!(authorize_update_install(true, "0.2.0", "0.2.0").is_ok());
    }
    #[test]
    fn updater_install_is_not_started_when_durable_handoff_fails() {
        let mut install_started = false;
        let result = install_after_durable_handoff(
            vec![1, 2, 3],
            || Err("marker fsync failed".to_owned()),
            |_| {
                install_started = true;
                Ok(())
            },
        );

        assert_eq!(result.unwrap_err(), "marker fsync failed");
        assert!(!install_started);
    }
    #[test]
    fn onboarding_binds_pinned_provider_to_fixed_cli_arguments() {
        let fixture_root = std::env::temp_dir().join("commonkit-desktop-onboarding");
        let request = OnboardingRequest {
            mode: "connect".into(),
            repository: "owner/kit".into(),
            kit_directory: fixture_root.join("kit"),
            loadout: "personal".into(),
            target: "workstation".into(),
            target_root: fixture_root.join("home"),
            provider: "apm".into(),
            provider_executable: Some(
                fixture_root.join(format!("apm{}", std::env::consts::EXE_SUFFIX)),
            ),
            provider_version: Some("0.25.0".into()),
            apm_manifest: Some("apm.yml".into()),
            apm_lockfile: Some("apm.lock.yaml".into()),
            apm_policy: Some("apm-policy.yml".into()),
            chezmoi_source: None,
            chezmoi_config: None,
            publish_registration: true,
        };
        let arguments = onboarding_arguments(&request).unwrap();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--provider-version", "0.25.0"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--apm-lockfile", "apm.lock.yaml"])
        );
    }

    #[test]
    fn onboarding_rejects_unpinned_or_traversing_provider_inputs() {
        let fixture_root = std::env::temp_dir().join("commonkit-desktop-onboarding-invalid");
        let request = OnboardingRequest {
            mode: "connect".into(),
            repository: "owner/kit".into(),
            kit_directory: fixture_root.join("kit"),
            loadout: "personal".into(),
            target: "workstation".into(),
            target_root: fixture_root.join("home"),
            provider: "chezmoi".into(),
            provider_executable: Some(
                fixture_root.join(format!("chezmoi{}", std::env::consts::EXE_SUFFIX)),
            ),
            provider_version: Some("latest".into()),
            apm_manifest: None,
            apm_lockfile: None,
            apm_policy: None,
            chezmoi_source: Some("../home".into()),
            chezmoi_config: Some("chezmoi.toml".into()),
            publish_registration: true,
        };
        assert!(onboarding_arguments(&request).is_err());
    }
}

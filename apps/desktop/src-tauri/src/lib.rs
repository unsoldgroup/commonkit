use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use commonkit_contracts::{Sha256Digest, StableId};
use commonkit_personal_context::{
    EncryptedRevisionStore, FieldOperation, ProfileFieldId, RevisionBinding, SecretValue,
    encrypt_revision_for_recipient_strings,
};
use commonkit_cli::about_me_setup::{SetupAnswers, SetupRequest, setup_profile};
use commonkit_core::{Principal, github_cli_executable};
use commonkit_platform::AppPaths;
use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, Rect, WindowEvent};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum GithubAuthStatus {
    SignedOut,
    Authenticated { login: String, method: &'static str },
}

fn github_auth_status_from(principal: Option<Principal>) -> GithubAuthStatus {
    match principal {
        Some(principal) => GithubAuthStatus::Authenticated {
            login: principal.as_str().to_owned(),
            method: "githubCli",
        },
        None => GithubAuthStatus::SignedOut,
    }
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
            Ok(development)
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
        install_managed_daemon(&runtime)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if service_is_authenticated(client) {
                return Ok(Self {
                    child: Mutex::new(None),
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
    #[cfg(test)]
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

fn install_managed_daemon(runtime: &RuntimeBinaryLayout) -> Result<(), DesktopError> {
    let status = Command::new(&runtime.cli)
        .args(["daemon", "install"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| DesktopError::ServiceLaunchFailed)?;
    if status.success() {
        Ok(())
    } else {
        Err(DesktopError::ServiceLaunchFailed)
    }
}

impl Drop for ServiceSupervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn service_is_authenticated(client: &ServiceClient) -> bool {
    let Ok(runtime) = RuntimeBinaryLayout::discover() else {
        return false;
    };
    let Ok(expected_digest) = binary_sha256(&runtime.daemon) else {
        return false;
    };
    tauri::async_runtime::block_on(client.status())
        .is_ok_and(|status| service_matches_runtime(&status, &expected_digest))
}

fn binary_sha256(path: &Path) -> Result<String, DesktopError> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn service_matches_runtime(status: &ServiceStatus, expected_digest: &str) -> bool {
    status.panel_contract_version == "commonkit.panel/v1"
        && status.daemon_binary_sha256 == expected_digest
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
    build_revision: String,
    daemon_binary_sha256: String,
    panel_contract_version: String,
    state: String,
    active_target: Option<String>,
    active_loadout: Option<String>,
    last_drift_check_unix_ms: Option<u128>,
    last_drift_error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PanelChannel {
    state: String,
    value: Option<u64>,
    source: String,
    observed_at_unix_ms: u128,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PanelSnapshot {
    contract_version: String,
    observed_at_unix_ms: u128,
    channels: BTreeMap<String, PanelChannel>,
}

impl PanelSnapshot {
    fn unavailable() -> Self {
        let channels = ["skills", "mcpServers", "changes", "credentials", "devices", "agentSessions", "drift"]
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    PanelChannel {
                        state: "unavailable".into(),
                        value: None,
                        source: "daemonPanel".into(),
                        observed_at_unix_ms: 0,
                        reason: Some("service_unavailable".into()),
                    },
                )
            })
            .collect();
        Self {
            contract_version: "commonkit.panel/v1".into(),
            observed_at_unix_ms: 0,
            channels,
        }
    }
}

impl ServiceStatus {
    fn offline() -> Self {
        Self {
            api_version: "v1".into(),
            contract_version: "1.0".into(),
            schema_version: 1,
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            build_revision: "unavailable".into(),
            daemon_binary_sha256: "unavailable".into(),
            panel_contract_version: "unavailable".into(),
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
    panel: PanelSnapshot,
    capabilities: Vec<CapabilityState>,
    last_event_id: Option<u64>,
    events: Vec<serde_json::Value>,
    git_sync: serde_json::Value,
    policy: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementSnapshot {
    about_me: serde_json::Value,
    plans: serde_json::Value,
    credentials: serde_json::Value,
    snapshots: serde_json::Value,
    relay: serde_json::Value,
    schedule: serde_json::Value,
    diagnostics: serde_json::Value,
    skills: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraySummary {
    health: String,
    computer: String,
    git_sync: String,
    policy: String,
    drift: String,
    relay: String,
    snapshots: String,
}

#[derive(Clone)]
struct TrayItems {
    health: MenuItem<tauri::Wry>,
    computer: MenuItem<tauri::Wry>,
    git_sync: MenuItem<tauri::Wry>,
    policy: MenuItem<tauri::Wry>,
    drift: MenuItem<tauri::Wry>,
    relay: MenuItem<tauri::Wry>,
    snapshots: MenuItem<tauri::Wry>,
}

impl TraySummary {
    fn from_observed(
        status: &ServiceStatus,
        relay: Option<&serde_json::Value>,
        snapshots: Option<&serde_json::Value>,
        diagnostics: Option<&serde_json::Value>,
    ) -> Self {
        let target = status.active_target.as_deref().unwrap_or("no target");
        let relay = relay
            .and_then(|value| value.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unavailable");
        let snapshots = snapshots
            .and_then(|value| value.get("snapshots"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::len);
        let git = diagnostics.and_then(|value| value.get("gitSync"));
        let git_state = git
            .and_then(|value| value.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unavailable");
        let ahead = git
            .and_then(|value| value.get("ahead"))
            .and_then(serde_json::Value::as_u64);
        let behind = git
            .and_then(|value| value.get("behind"))
            .and_then(serde_json::Value::as_u64);
        let git_sync = match (ahead, behind) {
            (Some(ahead), Some(behind)) => {
                format!("Git: {git_state} · {ahead} ahead · {behind} behind")
            }
            _ => format!("Git: {git_state}"),
        };
        let violations = diagnostics
            .and_then(|value| value.get("organizationPolicyViolations"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        Self {
            health: format!("Health: {}", status.state),
            computer: format!("Computer: {target}"),
            git_sync,
            policy: format!(
                "Policy: {violations} violation{}",
                if violations == 1 { "" } else { "s" }
            ),
            drift: status
                .last_drift_check_unix_ms
                .map(|value| format!("Last drift check: {value} ms since epoch"))
                .unwrap_or_else(|| "Last drift check: never".into()),
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
    project_loadout: Option<String>,
    target_override: Option<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeSetupAnswers {
    name: String,
    explanation_style: String,
    decision_style: String,
    tools: String,
    constraints: String,
    never_assume: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingDefaults {
    kit_directory: PathBuf,
    target_root: PathBuf,
    computer_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileEncryptRequest {
    binding: RevisionBinding,
    recipients: Vec<String>,
    /// `None` represents a signed field deletion. Strings exist only for this
    /// authorized local encryption command and are consumed into zeroizing memory.
    fields: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileEncryptionReceipt {
    revision_id: StableId,
    ciphertext_digest: Sha256Digest,
    staged: bool,
}

#[tauri::command]
fn encrypt_profile_revision(
    request: ProfileEncryptRequest,
) -> Result<ProfileEncryptionReceipt, DesktopError> {
    let fields = request
        .fields
        .into_iter()
        .map(|(field_id, value)| {
            let field_id = ProfileFieldId::parse(field_id).map_err(|_| DesktopError::InvalidInput)?;
            let operation = value.map_or(FieldOperation::Delete, |value| {
                FieldOperation::Set(SecretValue::from_string(value))
            });
            Ok((field_id, operation))
        })
        .collect::<Result<BTreeMap<_, _>, DesktopError>>()?;
    let revision_id = request.binding.revision_id.clone();
    let encrypted = encrypt_revision_for_recipient_strings(request.binding, fields, &request.recipients)
        .map_err(|_| DesktopError::ProfileEncryptionFailed)?;
    let paths = AppPaths::discover().map_err(|_| DesktopError::ServiceUnavailable)?;
    let store = EncryptedRevisionStore::open(paths.state.join("personal-context/staged"))
        .map_err(|_| DesktopError::ProfileEncryptionFailed)?;
    let ciphertext_digest = store
        .stage(&encrypted)
        .map_err(|error| match error {
            commonkit_personal_context::CryptoError::RevisionCollision => {
                DesktopError::ProfileRevisionCollision
            }
            _ => DesktopError::ProfileEncryptionFailed,
        })?;
    Ok(ProfileEncryptionReceipt {
        revision_id,
        ciphertext_digest,
        staged: true,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopSettingsSnapshot {
    autostart: bool,
    config_directory: PathBuf,
    state_directory: PathBuf,
    repository: Option<String>,
    target_root: Option<String>,
}

#[tauri::command]
fn desktop_settings_snapshot(
    app: tauri::AppHandle,
) -> Result<DesktopSettingsSnapshot, DesktopError> {
    let paths = AppPaths::discover().map_err(|_| DesktopError::ServiceUnavailable)?;
    let config = std::fs::read(paths.config.join("headless.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default();
    let sync = config.get("sync").unwrap_or(&serde_json::Value::Null);
    let repository = sync
        .pointer("/providerPipeline/source/trustedRemoteUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let target_root = sync
        .get("targetRoot")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let autostart = app
        .autolaunch()
        .is_enabled()
        .map_err(|_| DesktopError::ServiceUnavailable)?;
    Ok(DesktopSettingsSnapshot {
        autostart,
        config_directory: paths.config,
        state_directory: paths.state,
        repository,
        target_root,
    })
}

fn onboarding_defaults_from(
    paths: &AppPaths,
    home: Option<&Path>,
    computer_name: &str,
) -> Result<OnboardingDefaults, DesktopError> {
    let home = home.filter(|path| path.is_absolute());
    let target_root = home
        .map(|path| path.join("CommonKitManaged"))
        .unwrap_or_else(|| paths.config.join("managed-test"));
    let computer_name = if validate_id(computer_name).is_ok() {
        computer_name.to_owned()
    } else {
        "workstation".to_owned()
    };
    Ok(OnboardingDefaults {
        kit_directory: home
            .map(|path| path.join(".commonkit-kit"))
            .unwrap_or_else(|| paths.config.with_extension("kit")),
        target_root,
        computer_name,
    })
}

#[tauri::command]
fn onboarding_defaults() -> Result<OnboardingDefaults, DesktopError> {
    let paths = AppPaths::discover().map_err(|_| {
        DesktopError::OnboardingFailed(
            "CommonKit could not resolve safe local application folders".into(),
        )
    })?;
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let computer_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "workstation".to_owned())
        .to_ascii_lowercase();
    onboarding_defaults_from(&paths, home.as_deref(), &computer_name)
}

fn authorize_github_repository(
    mode: &str,
    repository: &str,
    status: &GithubAuthStatus,
) -> Result<(), DesktopError> {
    if mode != "create" {
        return Ok(());
    }
    let owner = repository
        .split_once('/')
        .map(|(owner, _)| owner)
        .ok_or(DesktopError::InvalidInput)?;
    match status {
        GithubAuthStatus::Authenticated { login, .. } if owner.eq_ignore_ascii_case(login) => {
            Ok(())
        }
        _ => Err(DesktopError::GithubAuthenticationFailed),
    }
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
    if let Some(project_loadout) = &request.project_loadout {
        validate_id(project_loadout)?;
    }
    if let Some(target_override) = &request.target_override {
        validate_id(target_override)?;
    }
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
    if let Some(project_loadout) = &request.project_loadout {
        args.extend(["--project-loadout".into(), project_loadout.clone()]);
    }
    if let Some(target_override) = &request.target_override {
        args.extend(["--target-override".into(), target_override.clone()]);
    }
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
    if request.mode == "create" {
        let status = github_auth_status_from(tauri::async_runtime::block_on(client.principal())?);
        authorize_github_repository(&request.mode, &request.repository, &status)?;
    }
    use commonkit_cli::onboarding::{
        InitMode, InitRequest, ProcessRunner, ProviderSelection, initialize,
    };
    // Fail before cloning, committing, or publishing anything when the
    // attached daemon cannot atomically adopt the resulting domains.
    tauri::async_runtime::block_on(client.json(reqwest::Method::GET, "/domains/reload", None))?;
    let paths = AppPaths::discover().map_err(|_| {
        DesktopError::OnboardingFailed(
            "CommonKit could not resolve safe local application folders".into(),
        )
    })?;
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
            project_loadout: request.project_loadout,
            target_override: request.target_override,
            target: request.target,
            target_root: request.target_root,
            config_directory: paths.config,
            state_directory: paths.state,
            provider,
            publish_registration: request.publish_registration,
        },
        &ProcessRunner::from_path(),
    )
    .map_err(|error| DesktopError::OnboardingFailed(error.to_string()))?;
    if tauri::async_runtime::block_on(client.json(
        reqwest::Method::POST,
        "/domains/reload",
        Some(serde_json::json!({"confirmed": true})),
    ))
    .is_err()
    {
        return Err(DesktopError::ServiceReloadFailed);
    }
    serde_json::to_value(result).map_err(Into::into)
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

    async fn panel(&self) -> Result<PanelSnapshot, DesktopError> {
        Ok(self
            .request(reqwest::Method::GET, "/panel")
            .await?
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn principal(&self) -> Result<Option<Principal>, DesktopError> {
        Ok(self
            .request(reqwest::Method::GET, "/principal")
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

#[cfg(test)]
fn management_routes() -> [(&'static str, &'static str); 8] {
    [
        ("about_me", "/about-me"),
        ("plans", "/compose"),
        ("credentials", "/credentials/readiness"),
        ("snapshots", "/snapshots"),
        ("relay", "/relay"),
        ("schedule", "/schedule"),
        ("diagnostics", "/diagnostics"),
        ("skills", "/skills"),
    ]
}

#[cfg(test)]
fn operator_routes() -> [(&'static str, &'static str); 15] {
    [
        ("about_me_reload", "/domains/reload"),
        ("about_me_decide", "/about-me/suggestions/decide"),
        ("plan", "/targets/{target}/sync/plan"),
        ("verify", "/targets/{target}/verify"),
        ("apply", "/targets/{target}/plans/{plan}/apply"),
        ("snapshot_create", "/snapshots"),
        ("snapshot_restore", "/snapshots/restore"),
        ("snapshot_promote", "/snapshots/promote"),
        ("relay_reconcile", "/relay/reconcile"),
        ("relay_restart", "/relay/restart"),
        ("schedule", "/schedule"),
        ("credential_plan", "/credentials/plan"),
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
        "CommonKit saved the new configuration, but its managed service could not reload it; reopen CommonKit and retry"
    )]
    ServiceReloadFailed,
    #[error("local control state is invalid")]
    InvalidControlState,
    #[error("input is invalid")]
    InvalidInput,
    #[error("GitHub CLI is unavailable; install GitHub CLI and retry")]
    GithubCliUnavailable,
    #[error("GitHub authentication failed; retry sign-in from CommonKit")]
    GithubAuthenticationFailed,
    #[error("CommonKit onboarding failed: {0}")]
    OnboardingFailed(String),
    #[error("CommonKit could not encrypt the profile revision locally")]
    ProfileEncryptionFailed,
    #[error("a different encrypted profile revision already uses this revision ID")]
    ProfileRevisionCollision,
    #[error("About Me setup could not be completed")]
    AboutMeSetupFailed,
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
async fn github_auth_status(
    client: tauri::State<'_, ServiceClient>,
) -> Result<GithubAuthStatus, DesktopError> {
    Ok(github_auth_status_from(client.principal().await?))
}

#[tauri::command]
async fn github_auth_login(
    client: tauri::State<'_, ServiceClient>,
) -> Result<GithubAuthStatus, DesktopError> {
    let succeeded = tauri::async_runtime::spawn_blocking(|| {
        let executable = github_cli_executable()?;
        Command::new(executable)
            .args([
                "auth",
                "login",
                "--hostname",
                "github.com",
                "--git-protocol",
                "https",
                "--web",
                "--clipboard",
                "--skip-ssh-key",
            ])
            .stdin(Stdio::null())
            .status()
            .map(|status| status.success())
    })
    .await
    .map_err(|_| DesktopError::GithubAuthenticationFailed)?
    .map_err(|_| DesktopError::GithubCliUnavailable)?;
    if !succeeded {
        return Err(DesktopError::GithubAuthenticationFailed);
    }
    let status = github_auth_status_from(client.principal().await?);
    match status {
        GithubAuthStatus::Authenticated { .. } => Ok(status),
        GithubAuthStatus::SignedOut => Err(DesktopError::GithubAuthenticationFailed),
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
    let panel = client
        .panel()
        .await
        .unwrap_or_else(|_| PanelSnapshot::unavailable());
    let event_snapshot = client
        .json(reqwest::Method::GET, "/events/snapshot", None)
        .await
        .unwrap_or_else(|_| serde_json::json!({"events": []}));
    let git_sync = match status.active_target.as_deref() {
        Some(target) => client
            .json(
                reqwest::Method::GET,
                &format!("/targets/{target}/git"),
                None,
            )
            .await
            .unwrap_or_else(unavailable),
        None => serde_json::json!({"state":"unavailable"}),
    };
    let policy = client
        .json(reqwest::Method::GET, "/policy/summary", None)
        .await
        .unwrap_or_else(unavailable);
    let online = status.state != "offline";
    Ok(DesktopSnapshot {
        status,
        panel,
        last_event_id: event_snapshot
            .get("lastEventId")
            .and_then(serde_json::Value::as_u64),
        events: event_snapshot
            .get("events")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default(),
        git_sync,
        policy,
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
    let about_me = client
        .json(reqwest::Method::GET, "/about-me", None)
        .await
        .unwrap_or_else(unavailable);
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
    let skills = client
        .json(reqwest::Method::GET, "/skills", None)
        .await
        .unwrap_or_else(unavailable);
    Ok(ManagementSnapshot {
        about_me,
        plans,
        credentials,
        snapshots,
        relay,
        schedule,
        diagnostics,
        skills,
    })
}

#[tauri::command]
async fn desktop_about_me_setup(
    client: tauri::State<'_, ServiceClient>,
    answers: AboutMeSetupAnswers,
    confirmed: bool,
) -> Result<serde_json::Value, DesktopError> {
    if !confirmed {
        return Err(DesktopError::InvalidInput);
    }
    let paths = AppPaths::discover().map_err(|_| DesktopError::ServiceUnavailable)?;
    let loadout = client
        .status()
        .await
        .ok()
        .and_then(|status| status.active_loadout)
        .unwrap_or_else(|| "personal".into());
    tauri::async_runtime::spawn_blocking(move || {
        setup_profile(SetupRequest {
            config_directory: paths.config,
            state_directory: paths.state,
            loadout_id: loadout,
            project_id: "default".into(),
            agent_id: "commonkit-agent".into(),
            answers: SetupAnswers {
                name: answers.name,
                explanation_style: answers.explanation_style,
                decision_style: answers.decision_style,
                tools: answers.tools,
                constraints: answers.constraints,
                never_assume: answers.never_assume,
            },
            approved: true,
        })
    })
    .await
    .map_err(|_| DesktopError::AboutMeSetupFailed)?
    .map_err(|_| DesktopError::AboutMeSetupFailed)?;
    client
        .json(
            reqwest::Method::POST,
            "/domains/reload",
            Some(serde_json::json!({"confirmed": true})),
        )
        .await
        .map_err(|_| DesktopError::ServiceReloadFailed)?;
    client
        .json(reqwest::Method::GET, "/about-me", None)
        .await
}

#[tauri::command]
async fn desktop_about_me_decide(
    client: tauri::State<'_, ServiceClient>,
    suggestion_id: String,
    decision: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_id(&suggestion_id)?;
    validate_id(&confirmation_id)?;
    if !matches!(decision.as_str(), "accept" | "reject") {
        return Err(DesktopError::InvalidInput);
    }
    client
        .json(
            reqwest::Method::POST,
            "/about-me/suggestions/decide",
            Some(serde_json::json!({
                "suggestionId": suggestion_id,
                "decision": decision,
                "confirmed": true,
                "confirmationId": confirmation_id
            })),
        )
        .await
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
async fn credential_plan(
    client: tauri::State<'_, ServiceClient>,
    destination_ids: Vec<String>,
) -> Result<serde_json::Value, DesktopError> {
    validate_ids(&destination_ids)?;
    client
        .json(
            reqwest::Method::POST,
            "/credentials/plan",
            Some(serde_json::json!({"destinationIds": destination_ids})),
        )
        .await
}

#[tauri::command]
async fn credential_apply(
    client: tauri::State<'_, ServiceClient>,
    plan_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    validate_digest(&plan_id)?;
    post_confirmed(
        &client,
        "/credentials/apply",
        serde_json::json!({"planId": plan_id}),
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

#[cfg(any(target_os = "windows", test))]
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
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Regular)
        .map_err(|error| error.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    let current = window.inner_size().map_err(|error| error.to_string())?;
    let current_width = f64::from(current.width) / scale;
    let current_height = f64::from(current.height) / scale;
    if current_width < 1_180.0 || current_height < 760.0 {
        window
            .set_size(tauri::LogicalSize::new(
                current_width.max(1_200.0),
                current_height.max(800.0),
            ))
            .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn hide_main_window(window: &tauri::Window) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    window
        .app_handle()
        .set_activation_policy(tauri::ActivationPolicy::Accessory)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The menu-bar panel. A native menu cannot draw the instrument face, so the tray icon
/// hangs this always-on-top popover instead. It stays read-only: open, or quit.
const TRAY_PANEL: &str = "tray-panel";

/// macOS dismisses the panel by blurring it before the tray click arrives; without this
/// the same click that closed the panel would immediately reopen it.
#[derive(Default)]
struct TrayPanelDismissal(Mutex<Option<Instant>>);

fn build_tray_panel(app: &tauri::AppHandle) -> Result<(), String> {
    tauri::WebviewWindowBuilder::new(
        app,
        TRAY_PANEL,
        tauri::WebviewUrl::App("tray.html".into()),
    )
    .title("CommonKit")
    .inner_size(360.0, 520.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .focused(false)
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn hide_tray_panel(window: &tauri::Window) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())
}

/// Anchor the panel under the tray icon the click reported, then show it.
fn toggle_tray_panel(app: &tauri::AppHandle, rect: Rect) -> Result<(), String> {
    let Some(window) = app.get_webview_window(TRAY_PANEL) else {
        return Ok(());
    };
    if window.is_visible().unwrap_or(false) {
        return window.hide().map_err(|error| error.to_string());
    }
    let dismissed = app
        .state::<TrayPanelDismissal>()
        .0
        .lock()
        .ok()
        .and_then(|at| *at)
        .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
    if dismissed {
        return Ok(());
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let anchor = rect.position.to_physical::<f64>(scale);
    let icon = rect.size.to_physical::<f64>(scale);
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let left = anchor.x + icon.width / 2.0 - f64::from(size.width) / 2.0;
    window
        .set_position(tauri::PhysicalPosition::new(
            left.round() as i32,
            (anchor.y + icon.height).round() as i32,
        ))
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn report_window_lifecycle_error(action: &str, result: Result<(), String>) {
    if let Err(error) = result {
        eprintln!("CommonKit could not {action}: {error}");
    }
}

/// The panel's only other control, and the same action the tray menu's Quit performs.
#[tauri::command]
fn quit_commonkit(app: tauri::AppHandle) {
    app.state::<ServiceSupervisor>().stop();
    app.exit(0);
}

async fn refresh_tray(app: tauri::AppHandle) {
    let client = app.state::<ServiceClient>().inner().clone();
    let mut status = client
        .status()
        .await
        .unwrap_or_else(|_| ServiceStatus::offline());
    if status.active_target.is_none()
        && let Ok(inventory) = client.json(reqwest::Method::GET, "/targets", None).await
    {
        status.active_target = selected_target(&inventory);
    }
    let relay = client.json(reqwest::Method::GET, "/relay", None).await.ok();
    let snapshots = client
        .json(reqwest::Method::GET, "/snapshots", None)
        .await
        .ok();
    let git_sync = match status.active_target.as_deref() {
        Some(target) => client
            .json(
                reqwest::Method::GET,
                &format!("/targets/{target}/git"),
                None,
            )
            .await
            .ok(),
        None => None,
    };
    let policy = client
        .json(reqwest::Method::GET, "/policy/summary", None)
        .await
        .ok();
    let peer = serde_json::json!({
        "gitSync": git_sync,
        "organizationPolicyViolations": policy.as_ref().and_then(|value| value.get("violations")).cloned().unwrap_or_else(|| serde_json::json!([]))
    });
    let summary =
        TraySummary::from_observed(&status, relay.as_ref(), snapshots.as_ref(), Some(&peer));
    let items = app.state::<TrayItems>();
    let _ = items.health.set_text(&summary.health);
    let _ = items.computer.set_text(&summary.computer);
    let _ = items.git_sync.set_text(&summary.git_sync);
    let _ = items.policy.set_text(&summary.policy);
    let _ = items.drift.set_text(&summary.drift);
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

fn selected_target(inventory: &serde_json::Value) -> Option<String> {
    let selected = inventory.get("selected")?.as_array()?;
    let id = selected.as_slice().first()?.as_str()?;
    if selected.len() == 1
        && inventory
            .get("targets")?
            .as_array()?
            .iter()
            .any(|target| target.get("id").and_then(serde_json::Value::as_str) == Some(id))
    {
        Some(id.to_owned())
    } else {
        None
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
            report_window_lifecycle_error("show the main window", show_main_window(app.clone()));
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
            github_auth_status,
            github_auth_login,
            onboarding_defaults,
            desktop_settings_snapshot,
            onboarding_initialize,
            encrypt_profile_revision,
            desktop_management_snapshot,
            desktop_about_me_setup,
            desktop_about_me_decide,
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
            credential_plan,
            credential_apply,
            credential_verify,
            diagnostics_export,
            set_autostart,
            check_for_update,
            install_update,
            show_main_window,
            quit_commonkit
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let supervisor = ServiceSupervisor::ensure_started(&client)?;
            app.manage(supervisor);
            let health = MenuItem::with_id(app, "health", "Health: starting", false, None::<&str>)?;
            let computer =
                MenuItem::with_id(app, "computer", "Computer: loading", false, None::<&str>)?;
            let relay =
                MenuItem::with_id(app, "relay-status", "Relay: loading", false, None::<&str>)?;
            let snapshots = MenuItem::with_id(
                app,
                "snapshot-status",
                "Snapshots: loading",
                false,
                None::<&str>,
            )?;
            let git_sync =
                MenuItem::with_id(app, "git-status", "Git: loading", false, None::<&str>)?;
            let policy =
                MenuItem::with_id(app, "policy-status", "Policy: loading", false, None::<&str>)?;
            let drift = MenuItem::with_id(
                app,
                "drift-status",
                "Last drift check: loading",
                false,
                None::<&str>,
            )?;
            let open = MenuItem::with_id(app, "open", "Open CommonKit", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            app.manage(TrayItems {
                health: health.clone(),
                computer: computer.clone(),
                git_sync: git_sync.clone(),
                policy: policy.clone(),
                drift: drift.clone(),
                relay: relay.clone(),
                snapshots: snapshots.clone(),
            });
            let menu = Menu::with_items(
                app,
                &[
                    &health, &computer, &git_sync, &policy, &drift, &relay, &snapshots, &open,
                    &quit,
                ],
            )?;
            app.manage(TrayPanelDismissal::default());
            build_tray_panel(app.handle()).map_err(std::io::Error::other)?;
            TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("CommonKit")
                .icon(Image::from_bytes(include_bytes!("../icons/tray-ck.png"))?)
                .icon_as_template(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        report_window_lifecycle_error(
                            "show the main window",
                            show_main_window(app.clone()),
                        );
                    }
                    "quit" => {
                        app.state::<ServiceSupervisor>().stop();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        report_window_lifecycle_error(
                            "show the status panel",
                            toggle_tray_panel(tray.app_handle(), rect),
                        );
                    }
                })
                .build(app)?;
            let start_minimized = std::env::args().any(|arg| arg == "--minimized");
            if !start_minimized {
                show_main_window(app.handle().clone()).map_err(std::io::Error::other)?;
            }
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
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == TRAY_PANEL => {
                api.prevent_close();
                report_window_lifecycle_error("hide the status panel", hide_tray_panel(window));
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                report_window_lifecycle_error("hide the main window", hide_main_window(window));
            }
            // The panel is a popover: losing focus dismisses it, as a menu would.
            WindowEvent::Focused(false) if window.label() == TRAY_PANEL => {
                if let Ok(mut dismissed) = window.state::<TrayPanelDismissal>().0.lock() {
                    *dismissed = Some(Instant::now());
                }
                report_window_lifecycle_error("hide the status panel", hide_tray_panel(window));
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build CommonKit desktop")
        .run(|app, event| {
            if let tauri::RunEvent::Reopen { .. } = event {
                report_window_lifecycle_error(
                    "reopen the main window",
                    show_main_window(app.clone()),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_defaults_are_computed_natively_without_webview_path_permissions() {
        let paths = AppPaths::from_roots("/private/config", "/private/data", "/private/cache")
            .expect("fixture roots");
        let defaults = onboarding_defaults_from(&paths, Some(Path::new("/Users/developer")), "local-workstation")
            .expect("defaults");

        assert_eq!(
            defaults.kit_directory,
            PathBuf::from("/Users/developer/.commonkit-kit")
        );
        assert_eq!(
            defaults.target_root,
            PathBuf::from("/Users/developer/CommonKitManaged")
        );
        assert_eq!(defaults.computer_name, "local-workstation");
    }

    #[test]
    fn onboarding_validation_errors_remain_actionable_in_the_gui() {
        let error = DesktopError::OnboardingFailed(
            "kit, configuration, state, and target roots must not overlap".into(),
        );
        assert_eq!(
            error.to_string(),
            "CommonKit onboarding failed: kit, configuration, state, and target roots must not overlap"
        );
    }

    #[test]
    fn repository_creation_is_bound_to_the_authenticated_github_owner() {
        let authenticated = GithubAuthStatus::Authenticated {
            login: "example-user".into(),
            method: "githubCli",
        };

        assert!(
            authorize_github_repository("create", "example-user/my-kit", &authenticated).is_ok()
        );
        assert!(
            authorize_github_repository("create", "someone-else/my-kit", &authenticated).is_err()
        );
        assert!(
            authorize_github_repository("connect", "shared-owner/my-kit", &authenticated).is_ok()
        );
        assert!(
            authorize_github_repository(
                "create",
                "example-user/my-kit",
                &GithubAuthStatus::SignedOut
            )
            .is_err()
        );
    }

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
            Some(&serde_json::json!({
                "gitSync": {"state":"diverged", "ahead":2, "behind":1},
                "organizationPolicyViolations": [{"code":"policy_required"}]
            })),
        );
        assert_eq!(summary.health, "Health: degraded");
        assert_eq!(summary.computer, "Computer: macbook");
        assert_eq!(summary.relay, "Relay: healthy");
        assert_eq!(summary.snapshots, "Snapshots: 2 available");
        assert_eq!(summary.git_sync, "Git: diverged · 2 ahead · 1 behind");
        assert_eq!(summary.policy, "Policy: 1 violation");
        assert_eq!(summary.drift, "Last drift check: never");
    }

    #[test]
    fn authenticated_service_must_match_the_bundled_daemon_identity() {
        let expected = format!("sha256:{}", "a".repeat(64));
        let status = ServiceStatus {
            daemon_binary_sha256: expected.clone(),
            panel_contract_version: "commonkit.panel/v1".into(),
            ..ServiceStatus::offline()
        };

        assert!(service_matches_runtime(&status, &expected));
        assert!(!service_matches_runtime(
            &status,
            &format!("sha256:{}", "b".repeat(64))
        ));
    }

    #[test]
    fn tray_uses_the_one_persisted_selected_target() {
        assert_eq!(
            selected_target(&serde_json::json!({
                "selected": ["local-workstation"],
                "targets": [{"id": "local-workstation"}]
            })),
            Some("local-workstation".into())
        );
        assert_eq!(
            selected_target(&serde_json::json!({
                "selected": ["one", "two"],
                "targets": [{"id": "one"}, {"id": "two"}]
            })),
            None
        );
    }

    #[test]
    fn tray_icon_has_a_monochrome_vector_source() {
        let icon = include_str!("../icons/tray-ck.svg");

        assert!(icon.contains("viewBox=\"0 0 24 24\""));
        assert!(icon.contains("fill=\"#000000\""));
        assert!(!icon.contains("<text"));
        assert!(!icon.contains("stroke="));
    }

    #[test]
    fn management_snapshot_uses_only_fixed_read_only_service_routes() {
        assert_eq!(
            management_routes(),
            [
                ("about_me", "/about-me"),
                ("plans", "/compose"),
                ("credentials", "/credentials/readiness"),
                ("snapshots", "/snapshots"),
                ("relay", "/relay"),
                ("schedule", "/schedule"),
                ("diagnostics", "/diagnostics"),
                ("skills", "/skills"),
            ]
        );
    }
    #[test]
    fn operator_actions_are_bound_to_fixed_service_routes_and_methods() {
        assert_eq!(
            operator_routes(),
            [
                ("about_me_reload", "/domains/reload"),
                ("about_me_decide", "/about-me/suggestions/decide"),
                ("plan", "/targets/{target}/sync/plan"),
                ("verify", "/targets/{target}/verify"),
                ("apply", "/targets/{target}/plans/{plan}/apply"),
                ("snapshot_create", "/snapshots"),
                ("snapshot_restore", "/snapshots/restore"),
                ("snapshot_promote", "/snapshots/promote"),
                ("relay_reconcile", "/relay/reconcile"),
                ("relay_restart", "/relay/restart"),
                ("schedule", "/schedule"),
                ("credential_plan", "/credentials/plan"),
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
            project_loadout: Some("project-web".into()),
            target_override: Some("target-workstation".into()),
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
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--project-loadout", "project-web"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--target-override", "target-workstation"])
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
            project_loadout: None,
            target_override: None,
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

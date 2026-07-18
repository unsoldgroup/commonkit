use std::path::PathBuf;
use std::time::Duration;

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSummary {
    current_version: String,
    version: String,
    notes: Option<String>,
    published_at: Option<String>,
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
        plan_id: &str,
        confirmation_id: &str,
    ) -> Result<serde_json::Value, DesktopError> {
        validate_digest(plan_id)?;
        validate_id(confirmation_id)?;
        let key = format!("desktop-{}", &plan_id[7..23]);
        Ok(self
            .request(reqwest::Method::POST, &format!("/plans/{plan_id}/apply"))
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
    #[error("local control state is invalid")]
    InvalidControlState,
    #[error("input is invalid")]
    InvalidInput,
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
    plan_id: String,
    confirmation_id: String,
) -> Result<serde_json::Value, DesktopError> {
    client.apply(&plan_id, &confirmation_id).await
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

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

pub fn run() {
    let client = ServiceClient::discover().unwrap_or_else(|_| ServiceClient {
        discovery: PathBuf::new(),
        token: PathBuf::new(),
        http: reqwest::Client::new(),
    });
    tauri::Builder::default()
        .manage(client)
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show_main_window(app.clone());
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--minimized"])
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            desktop_snapshot,
            desktop_management_snapshot,
            apply_plan,
            set_autostart,
            check_for_update,
            install_update,
            show_main_window
        ])
        .setup(|app| {
            let open = MenuItem::with_id(app, "open", "Open CommonKit", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("CommonKit")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        let _ = show_main_window(app.clone());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
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
    fn validates_plan_bound_command_inputs() {
        assert!(validate_digest(&format!("sha256:{}", "a".repeat(64))).is_ok());
        assert!(validate_digest("../../control.token").is_err());
        assert!(validate_id("desktop-confirmation-1").is_ok());
        assert!(validate_id("bad/value").is_err());
    }
    #[test]
    fn update_install_requires_consent_bound_to_the_observed_version() {
        assert!(authorize_update_install(false, "0.2.0", "0.2.0").is_err());
        assert!(authorize_update_install(true, "0.2.0", "0.3.0").is_err());
        assert!(authorize_update_install(true, "0.2.0", "0.2.0").is_ok());
    }
}

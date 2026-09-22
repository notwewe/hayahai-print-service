mod agent;
mod api;
mod model;
mod printer;
mod protocol;
mod storage;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use agent::AgentRuntime;
use api::ApiClient;
use base64::{Engine, engine::general_purpose::STANDARD};
use model::{AgentConfig, AgentStatus, EnrollRequest, PROTOCOL};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, State,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_updater::UpdaterExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("The print service has not been paired")]
    NotPaired,
    #[error("The pairing link is invalid")]
    InvalidPairing,
    #[error("This operating system is not supported")]
    UnsupportedPlatform,
    #[error("API error: {0}")]
    Api(String),
    #[error("Printer error: {0}")]
    Printer(String),
    #[error("Security error: {0}")]
    Security(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Keyring(#[from] keyring::Error),
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairResult {
    agent_id: String,
    api_url: String,
}

#[tauri::command]
async fn status(runtime: State<'_, AgentRuntime>) -> Result<AgentStatus, String> {
    Ok(runtime.status.read().await.clone())
}

#[tauri::command]
async fn pair(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
    pairing_uri: String,
) -> Result<PairResult, String> {
    let uri = url::Url::parse(&pairing_uri).map_err(|_| AgentError::InvalidPairing.to_string())?;
    if uri.scheme() != "hayahai-print" || uri.host_str() != Some("pair") {
        return Err(AgentError::InvalidPairing.to_string());
    }
    let value = |key: &str| {
        uri.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
            .ok_or(AgentError::InvalidPairing)
    };
    let api_url = value("api").map_err(|error| error.to_string())?;
    let enrollment_id = value("enrollment").map_err(|error| error.to_string())?;
    let secret = value("secret").map_err(|error| error.to_string())?;
    api::validate_api_url(&api_url).map_err(|error| error.to_string())?;
    // A fresh key makes an explicit re-pair independent of a revoked or stale identity.
    let key = storage::new_signing_key();
    let request = EnrollRequest {
        enrollment_id,
        secret,
        public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        hostname: hostname::get()
            .map_err(AgentError::Io)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned(),
        username: Some(whoami::username()),
        operating_system: if cfg!(target_os = "windows") {
            "windows"
        } else {
            "macos"
        }
        .into(),
        architecture: std::env::consts::ARCH.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: PROTOCOL.into(),
    };
    let response = ApiClient::enroll(&api_url, &request)
        .await
        .map_err(|error| error.to_string())?;
    let config = AgentConfig {
        api_url: api_url.trim_end_matches('/').into(),
        agent_id: response.id.clone(),
        tenant_id: response.tenant_id,
        protocol: response.protocol,
        counter: 0,
    };
    // Follow the same lock order as signed requests so credentials change atomically in memory.
    let mut config_guard = runtime.config.lock().await;
    storage::save_signing_key(&key).map_err(|error| error.to_string())?;
    storage::save_config(&app, &config).map_err(|error| error.to_string())?;
    *runtime.signing_key.write().await = key;
    *config_guard = Some(config.clone());
    drop(config_guard);
    *runtime.status.write().await = AgentStatus {
        paired: true,
        api_url: Some(config.api_url.clone()),
        agent_id: Some(config.agent_id.clone()),
        printers: Vec::new(),
        active_job: false,
        last_error: None,
        version: env!("CARGO_PKG_VERSION").into(),
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    Ok(PairResult {
        agent_id: config.agent_id,
        api_url: config.api_url,
    })
}

fn emit_pairing(app: &AppHandle, values: impl IntoIterator<Item = String>) {
    for value in values {
        if value.starts_with("hayahai-print://pair?") {
            let _ = app.emit("pair-request", value);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            break;
        }
    }
}

fn tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Status and pairing", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Open logs", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &logs, &quit])?;
    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("HayahAI Print Service");
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "logs" => {
                if let Ok(path) = app.path().app_log_dir() {
                    let _ = tauri_plugin_opener::open_path(path, None::<&str>);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

async fn updater_loop(app: AppHandle, active_job: Arc<AtomicBool>) {
    loop {
        if !active_job.load(Ordering::SeqCst) {
            match app.updater() {
                Ok(updater) => match updater.check().await {
                    Ok(Some(update)) => {
                        if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
                            log::warn!("Update installation failed: {error}");
                        } else {
                            app.restart();
                        }
                    }
                    Ok(None) => {}
                    Err(error) => log::warn!("Update check failed: {error}"),
                },
                Err(error) => log::warn!("Updater configuration failed: {error}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            emit_pairing(app, args);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .max_file_size(1_000_000)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri_plugin_autostart::ManagerExt;
            use tauri_plugin_deep_link::DeepLinkExt;

            let handle = app.handle().clone();
            let config = storage::load_config(&handle)?;
            let paired = config.is_some();
            let key = storage::signing_key()?;
            let runtime = AgentRuntime::new(config, key);
            let api = ApiClient::new(
                handle.clone(),
                runtime.signing_key.clone(),
                runtime.config.clone(),
            )?;
            app.manage(runtime.clone());
            tray(&handle)?;
            let event_handle = handle.clone();
            app.deep_link().on_open_url(move |event| {
                emit_pairing(
                    &event_handle,
                    event.urls().into_iter().map(|url| url.to_string()),
                );
            });
            if let Some(urls) = app.deep_link().get_current()? {
                emit_pairing(&handle, urls.into_iter().map(|url| url.to_string()));
            }
            if let Err(error) = app.autolaunch().enable() {
                log::warn!("Could not enable login startup: {error}");
            }
            if paired && let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
            tauri::async_runtime::spawn(agent::run(handle, runtime, api));
            tauri::async_runtime::spawn(updater_loop(
                app.handle().clone(),
                app.state::<AgentRuntime>().active_job.clone(),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![status, pair])
        .run(tauri::generate_context!())
        .expect("HayahAI Print Service failed");
}

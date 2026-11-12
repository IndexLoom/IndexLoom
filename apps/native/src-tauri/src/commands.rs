//! Native mode chooser. Do not expose its commands to remote webpages.
use serde::Serialize;
use tauri::Manager;
use tokio::sync::Mutex;
use indexloom_core::{deployment::validate_remote_url, platform};

use super::secrets;

#[derive(Default)]
pub struct NativeState {
    session: Mutex<Option<LocalSession>>,
}

struct LocalSession {
    runtime: indexloom_server::local::LocalInstance,
    operator_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonomousEndpoint {
    url: String,
    operator_token: String,
}

fn require_packaged_shell(window: &tauri::WebviewWindow) -> Result<(), String> {
    let origin = window.url().map_err(|_| "could not verify native page origin")?;
    let allowed = (origin.scheme() == "tauri" && origin.host_str() == Some("localhost"))
        || (origin.scheme() == "http" && origin.host_str() == Some("tauri.localhost"));
    if allowed { Ok(()) } else { Err("native mode controls are only available in the packaged shell".into()) }
}

#[tauri::command]
pub fn platform_info(window: tauri::WebviewWindow) -> Result<String, String> {
    require_packaged_shell(&window)?;
    Ok(format!("{:?}", platform::current().platform))
}

#[tauri::command]
pub fn validate_external(window: tauri::WebviewWindow, url: String) -> Result<String, String> {
    require_packaged_shell(&window)?;
    validate_remote_url(&url).map(|site| site.to_string())
}

#[tauri::command]
pub async fn start_autonomous(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, NativeState>,
) -> Result<AutonomousEndpoint, String> {
    require_packaged_shell(&window)?;
    let mut session = state.session.lock().await;
    if let Some(current) = session.as_ref() {
        return Ok(AutonomousEndpoint {
            url: current.runtime.url(),
            operator_token: current.operator_token.clone(),
        });
    }
    let root = app.path().app_local_data_dir()
        .map_err(|_| "native app data directory is unavailable")?;
    let secrets = secrets::load_or_create(&root)?;
    let runtime = indexloom_server::local::start(
        &root.join("indexloom.db"),
        &secrets.master_key,
        &secrets.operator_token,
    ).await.map_err(|error| format!("local IndexLoom cannot start: {error:#}"))?;
    let response = AutonomousEndpoint {
        url: runtime.url(),
        operator_token: secrets.operator_token.clone(),
    };
    *session = Some(LocalSession { runtime, operator_token: secrets.operator_token });
    Ok(response)
}

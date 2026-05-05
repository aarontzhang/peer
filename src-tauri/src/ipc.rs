use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::db::Recording;
use crate::hotkey::HotkeyStatus;
use crate::recording;
use crate::state::AppState;

/// Read API keys from `<app_data_dir>/keys.json` if present. Returns
/// `(openai, anthropic)`. Either may be `None`.
fn read_keys_file(app: &AppHandle) -> (Option<String>, Option<String>) {
    let Ok(dir) = app.path().app_data_dir() else {
        return (None, None);
    };
    let path = dir.join("keys.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (None, None);
    };
    let pick = |k: &str| {
        value
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    (pick("openai"), pick("anthropic"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyStatus {
    pub openai: bool,
    pub anthropic: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyArgs {
    pub provider: String, // "openai" | "anthropic"
    pub key: String,
}

fn err_to_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    recording::start(app, state.inner().clone())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    recording::stop(app, state.inner().clone())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn cancel_recording(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    recording::cancel(app, state.inner().clone())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn send_recording(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    recording::send(app, state.inner().clone())
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub async fn list_recordings(state: State<'_, Arc<AppState>>) -> Result<Vec<Recording>, String> {
    state.db().list_recordings(200).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn get_recording(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<Recording>, String> {
    state.db().get_recording(&id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn delete_recording(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    if let Some(rec) = state.db().get_recording(&id).await.map_err(err_to_string)? {
        purge_artifacts(state.inner(), &rec.id, &rec.video_path).await;
    }
    state
        .db()
        .delete_recording(&id)
        .await
        .map_err(err_to_string)
}

/// Best-effort cleanup of on-disk artifacts: video, sibling mp3, and the
/// per-recording frames directory. Errors are swallowed — the DB row is
/// already gone and we don't want orphan files to block deletion.
async fn purge_artifacts(state: &Arc<AppState>, id: &str, video_path: &str) {
    let video = std::path::PathBuf::from(video_path);
    let _ = tokio::fs::remove_file(&video).await;
    let _ = tokio::fs::remove_file(video.with_extension("mp3")).await;
    let _ = tokio::fs::remove_dir_all(state.frames_dir.join(id)).await;
}

#[tauri::command]
pub fn open_result_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("result") {
        win.show().map_err(err_to_string)?;
        win.set_focus().map_err(err_to_string)?;
    }
    Ok(())
}

#[tauri::command]
pub fn move_pill(app: AppHandle, x: f64, y: f64) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("pill") {
        win.set_position(tauri::LogicalPosition::new(x, y))
            .map_err(err_to_string)?;
    }
    Ok(())
}

/// Global cursor position in macOS user-space (top-left origin, points).
/// Lives in the same coordinate space as `LogicalPosition`, so JS can
/// subtract the pill window's logical position directly. Polled on rAF
/// from the pill so the glasses can follow the mouse.
#[tauri::command]
pub fn cursor_position() -> Result<[f64; 2], String> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "CGEventSource::new failed".to_string())?;
    let event = CGEvent::new(src).map_err(|_| "CGEvent::new failed".to_string())?;
    let loc = event.location();
    Ok([loc.x, loc.y])
}

#[tauri::command]
pub fn set_api_key(args: SetApiKeyArgs) -> Result<(), String> {
    let service = "Peer";
    let account = match args.provider.as_str() {
        "openai" => "openai-api-key",
        "anthropic" => "anthropic-api-key",
        other => return Err(format!("unknown provider: {other}")),
    };
    let entry = keyring::Entry::new(service, account).map_err(err_to_string)?;
    entry.set_password(&args.key).map_err(err_to_string)?;
    Ok(())
}

#[tauri::command]
pub fn get_hotkey_status(state: State<'_, Arc<AppState>>) -> Result<HotkeyStatus, String> {
    Ok(state.hotkey_status.lock().clone())
}

#[tauri::command]
pub fn get_api_key_status(app: AppHandle) -> Result<ApiKeyStatus, String> {
    Ok(ApiKeyStatus {
        openai: read_api_key(&app, "openai").is_some(),
        anthropic: read_api_key(&app, "anthropic").is_some(),
    })
}

/// Resolve an API key for `provider`. Order:
///   1. `<app_data>/keys.json` — what the user typed in Settings, authoritative
///   2. macOS Keychain — legacy Settings dialog
///   3. Process env (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`) — dev fallback
///
/// Settings wins over env so a stale `ANTHROPIC_API_KEY` in a user's shell
/// can't silently override the key they just entered in the app.
pub fn read_api_key(app: &AppHandle, provider: &str) -> Option<String> {
    let (env_var, account) = match provider {
        "openai" => ("OPENAI_API_KEY", "openai-api-key"),
        "anthropic" => ("ANTHROPIC_API_KEY", "anthropic-api-key"),
        _ => return None,
    };
    let (openai, anthropic) = read_keys_file(app);
    let from_file = match provider {
        "openai" => openai,
        "anthropic" => anthropic,
        _ => None,
    };
    if from_file.is_some() {
        return from_file;
    }
    if let Some(v) = keyring::Entry::new("Peer", account)
        .and_then(|e| e.get_password())
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Some(v);
    }
    std::env::var(env_var).ok().filter(|s| !s.is_empty())
}

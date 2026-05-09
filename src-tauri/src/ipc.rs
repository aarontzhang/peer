use std::sync::Arc;

use tauri::{AppHandle, Manager, State};

use crate::db::Recording;
use crate::hotkey::{self, HotkeyStatus, RecordingKeybind};
use crate::recording;
use crate::reveal_result_window;
use crate::saas::{self, AccountStatus, SetDeviceTokenArgs};
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
            .map(str::trim)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    (pick("openai"), pick("anthropic"))
}

/// Drop a provider entry from `<app_data_dir>/keys.json`. Used as
/// defensive cleanup when a wrong-provider key is detected on read,
/// so the same bad value doesn't keep getting silently filtered on
/// every recording.
fn purge_keys_file_entry(app: &AppHandle, provider: &str) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let path = dir.join("keys.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    if map.remove(provider).is_some() {
        if let Ok(new_bytes) = serde_json::to_vec_pretty(&value) {
            let _ = std::fs::write(&path, new_bytes);
        }
    }
}

fn purge_keychain_entry(provider: &str) {
    let account = match provider {
        "openai" => "openai-api-key",
        "anthropic" => "anthropic-api-key",
        _ => return,
    };
    if let Ok(entry) = keyring::Entry::new("Peer", account) {
        let _ = entry.delete_credential();
    }
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
    reveal_result_window(&app, true).map_err(err_to_string)
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
pub fn get_hotkey_status(state: State<'_, Arc<AppState>>) -> Result<HotkeyStatus, String> {
    Ok(state.hotkey_status.lock().clone())
}

#[tauri::command]
pub fn set_recording_keybind(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    keybind: RecordingKeybind,
) -> Result<HotkeyStatus, String> {
    hotkey::set_recording_keybind(app, state.inner().clone(), keybind).map_err(err_to_string)
}

#[tauri::command]
pub fn get_account_status() -> Result<AccountStatus, String> {
    Ok(saas::account_status())
}

#[tauri::command]
pub fn open_account_login(app: AppHandle) -> Result<String, String> {
    saas::open_login(&app).map_err(err_to_string)
}

#[tauri::command]
pub fn set_device_token(args: SetDeviceTokenArgs) -> Result<(), String> {
    saas::set_device_token(args).map_err(err_to_string)
}

#[tauri::command]
pub fn sign_out() -> Result<(), String> {
    saas::sign_out().map_err(err_to_string)
}

/// Resolve an API key for `provider`. Order:
///   1. `<app_data>/keys.json` — what the user typed in Settings, authoritative
///   2. macOS Keychain — legacy Settings dialog
///   3. Process env (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`) — dev fallback
///
/// Settings wins over env so a stale key in a user's shell can't silently
/// override the key they just entered in the app. Obvious cross-provider keys
/// are skipped so `OPENAI_API_KEY=sk-ant-...` never reaches Whisper.
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
    if let Some(raw) = from_file.as_deref() {
        if is_wrong_provider_key(provider, raw) {
            tracing::warn!(
                provider,
                "purging cross-provider key from keys.json (was a {} key)",
                key_provider_label(raw)
            );
            purge_keys_file_entry(app, provider);
        } else if let Some(v) = clean_provider_key(provider, from_file.clone()) {
            return Some(v);
        }
    }
    let from_keyring = keyring::Entry::new("Peer", account)
        .and_then(|e| e.get_password())
        .ok();
    if let Some(raw) = from_keyring.as_deref() {
        if is_wrong_provider_key(provider, raw) {
            tracing::warn!(
                provider,
                "purging cross-provider key from Keychain (was a {} key)",
                key_provider_label(raw)
            );
            purge_keychain_entry(provider);
        } else if let Some(v) = clean_provider_key(provider, from_keyring.clone()) {
            return Some(v);
        }
    }
    clean_provider_key(provider, std::env::var(env_var).ok())
}

fn clean_provider_key(provider: &str, key: Option<String>) -> Option<String> {
    key.map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|s| !is_wrong_provider_key(provider, s))
}

fn is_wrong_provider_key(provider: &str, key: &str) -> bool {
    match provider {
        "openai" => key.starts_with("sk-ant-"),
        "anthropic" => {
            key.starts_with("sk-proj-") || (key.starts_with("sk-") && !key.starts_with("sk-ant-"))
        }
        _ => false,
    }
}

fn key_provider_label(key: &str) -> &'static str {
    if key.starts_with("sk-ant-") {
        "Anthropic"
    } else if key.starts_with("sk-proj-") || key.starts_with("sk-") {
        "OpenAI"
    } else {
        "Provider"
    }
}

#[cfg(test)]
mod tests {
    use super::is_wrong_provider_key;

    #[test]
    fn rejects_cross_provider_keys() {
        assert!(is_wrong_provider_key("openai", "sk-ant-example"));
        assert!(is_wrong_provider_key("anthropic", "sk-proj-example"));
        assert!(is_wrong_provider_key("anthropic", "sk-example"));

        assert!(!is_wrong_provider_key("openai", "sk-proj-example"));
        assert!(!is_wrong_provider_key("openai", "sk-example"));
        assert!(!is_wrong_provider_key("anthropic", "sk-ant-example"));
    }
}

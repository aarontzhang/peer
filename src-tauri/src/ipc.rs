use std::sync::Arc;

use tauri::{AppHandle, Manager, State};

use crate::automation;
use crate::db::Recording;
use crate::hotkey::{self, HotkeyStatus, PermissionMode, RecordingKeybind};
use crate::recording;
use crate::reveal_result_window;
use crate::saas::{self, AccountStatus};
use crate::state::AppState;

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
pub async fn retry_recording(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    recording::retry(app, state.inner().clone(), id)
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
pub fn get_permission_mode(state: State<'_, Arc<AppState>>) -> Result<PermissionMode, String> {
    Ok(*state.permission_mode.lock())
}

#[tauri::command]
pub fn set_permission_mode(
    state: State<'_, Arc<AppState>>,
    mode: PermissionMode,
) -> Result<PermissionMode, String> {
    hotkey::set_permission_mode(state.inner().clone(), mode).map_err(err_to_string)
}

#[tauri::command]
pub fn get_session() -> Result<AccountStatus, String> {
    Ok(saas::account_status())
}

#[tauri::command]
pub fn start_google_sign_in(app: AppHandle) -> Result<String, String> {
    saas::open_login(&app).map_err(err_to_string)
}

#[tauri::command]
pub fn sign_out(app: AppHandle) -> Result<(), String> {
    saas::sign_out(&app).map_err(err_to_string)
}

#[tauri::command]
pub async fn run_automation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    automation::start(app, state.inner().clone(), id)
        .await
        .map_err(err_to_string)
}

#[tauri::command]
pub fn cancel_automation(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    automation::cancel(state.inner().clone());
    Ok(())
}


use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::chat::{self, ChatThreadEntry};
use crate::db::{Recording, RecordingMessage, RecordingVersion, VersionSource};
use crate::hotkey::{self, HotkeyStatus, PermissionMode, RecordingKeybind};
use crate::pipeline::{ChunkKind as ResultChunkKind, ResultChunk};
use crate::recording;
use crate::reveal_result_window;
use crate::saas::{self, AccountStatus, SaasClient};
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
pub async fn list_versions(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Vec<RecordingVersion>, String> {
    state.db().list_versions(&id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn get_version(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<RecordingVersion>, String> {
    state.db().get_version(&id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn revert_to_version(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<RecordingVersion, String> {
    let target = state
        .db()
        .get_version(&id)
        .await
        .map_err(err_to_string)?
        .ok_or_else(|| "version not found".to_string())?;

    // Append-only: copy the chosen version's body forward as a brand-new
    // `revert` row. Newer versions (and the chat thread that produced them)
    // stay intact so the user can step forward again.
    let new_version = state
        .db()
        .append_version(
            &target.recording_id,
            VersionSource::Revert,
            &target.body,
            None,
            None,
            Some(&target.id),
        )
        .await
        .map_err(err_to_string)?;

    // Re-use the existing typewriter pathway so the prompt pane animates
    // the swap. Begin + End is enough — clients accumulate to End.text.
    let _ = app.emit(
        "result:chunk",
        &ResultChunk {
            id: target.recording_id.clone(),
            kind: ResultChunkKind::Begin,
            text: String::new(),
        },
    );
    let _ = app.emit(
        "result:chunk",
        &ResultChunk {
            id: target.recording_id.clone(),
            kind: ResultChunkKind::End,
            text: new_version.body.clone(),
        },
    );

    Ok(new_version)
}

#[tauri::command]
pub async fn get_chat_thread(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Vec<RecordingMessage>, String> {
    state.db().get_chat_thread(&id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn send_chat_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    content: String,
) -> Result<RecordingMessage, String> {
    let content_trimmed = content.trim().to_string();
    if content_trimmed.is_empty() {
        return Err("chat message is empty".into());
    }

    // Pull the recording up front so we can reject obviously-bad states
    // (missing body, mid-pipeline) before the user sees a streaming bubble.
    let rec = state
        .db()
        .get_recording(&id)
        .await
        .map_err(err_to_string)?
        .ok_or_else(|| "recording not found".to_string())?;

    if rec.body.as_deref().unwrap_or("").trim().is_empty() {
        return Err(
            "this recording has no prompt yet — wait for it to finish analyzing first".into(),
        );
    }

    let backend = SaasClient::from_keychain(app.clone())
        .await
        .ok_or_else(|| "Sign in to Peer to use chat refinement".to_string())?;

    // Persist the user's message immediately so the dock can show it
    // optimistically and a refresh during streaming still picks it up.
    let user_msg = state
        .db()
        .insert_user_message(&id, &content_trimmed)
        .await
        .map_err(err_to_string)?;

    let thread_entries: Vec<ChatThreadEntry> = state
        .db()
        .get_chat_thread(&id)
        .await
        .map_err(err_to_string)?
        .into_iter()
        // Exclude the just-inserted user turn — it's the `newMessage`, not
        // part of the prior thread.
        .filter(|m| m.id != user_msg.id)
        .map(|m| ChatThreadEntry {
            role: m.role,
            content: m.content,
        })
        .collect();

    let current_body = rec.body.clone().unwrap_or_default();
    let mode = state.permission_mode.lock().as_str().to_string();

    // Turn id is opaque to the backend; the frontend uses it to demultiplex
    // chat:chunk events when several chats overlap.
    let turn_id = Uuid::new_v4().to_string();

    let app_bg = app.clone();
    let state_bg = state.inner().clone();
    let recording_id = id.clone();
    let user_msg_id = user_msg.id.clone();
    let new_message = content_trimmed.clone();
    tokio::spawn(async move {
        let result = chat::stream_chat_turn(
            app_bg.clone(),
            &backend,
            chat::ChatRequest {
                recording_id: &recording_id,
                turn_id: &turn_id,
                current_body: &current_body,
                mode: &mode,
                thread: &thread_entries,
                new_message: &new_message,
            },
        )
        .await;

        match result {
            Ok(assistant_text) => {
                let body_clean = assistant_text.trim();
                if body_clean.is_empty() {
                    tracing::warn!(recording_id = %recording_id, "chat stream returned empty body");
                    return;
                }
                match state_bg
                    .db()
                    .append_version(
                        &recording_id,
                        VersionSource::Chat,
                        body_clean,
                        None,
                        Some(&user_msg_id),
                        None,
                    )
                    .await
                {
                    Ok(version) => {
                        if let Err(err) = state_bg
                            .db()
                            .insert_assistant_message(
                                &recording_id,
                                body_clean,
                                Some(&version.id),
                            )
                            .await
                        {
                            tracing::warn!(?err, "failed to persist assistant message");
                        }
                        let _ = app_bg.emit("chat:turn-complete", &serde_json::json!({
                            "recordingId": recording_id,
                            "versionId": version.id,
                        }));
                    }
                    Err(err) => {
                        tracing::error!(?err, "failed to append chat version");
                        let _ = app_bg.emit("chat:error", &serde_json::json!({
                            "recordingId": recording_id,
                            "message": format!("{err:#}"),
                        }));
                    }
                }
            }
            Err(err) => {
                tracing::error!(?err, "chat streaming failed");
                let _ = app_bg.emit("chat:error", &serde_json::json!({
                    "recordingId": recording_id,
                    "message": format!("{err:#}"),
                }));
            }
        }
    });

    Ok(user_msg)
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


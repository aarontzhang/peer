//! Recording lifecycle: drives the Swift sidecar, owns timing, hands off to the pipeline.

mod capture;
mod cursor;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::db::{Recording, RecordingStatus};
use crate::pipeline;
use crate::state::AppState;

pub use capture::CaptureProcess;

// Hard cap as a safety net so a forgotten recording can't fill the disk.
// Display shows elapsed only — no countdown.
const MAX_DURATION_MS: u64 = 3 * 60 * 1000;

#[derive(Default)]
pub struct RecordingController;

pub struct ActiveRecording {
    pub id: String,
    pub video_path: PathBuf,
    pub started_at: Instant,
    pub capture: CaptureProcess,
    pub auto_stop_handle: Option<tokio::task::JoinHandle<()>>,
}

/// A recording whose capture has stopped but which is awaiting user
/// confirmation (send) or rejection (cancel). The video is on disk; the
/// pipeline has not yet started.
pub struct ReviewRecording {
    pub id: String,
    pub video_path: PathBuf,
    pub duration_ms: u64,
}

pub enum RecordingPhase {
    Active(ActiveRecording),
    Review(ReviewRecording),
}

impl RecordingPhase {
    pub fn id(&self) -> &str {
        match self {
            RecordingPhase::Active(a) => &a.id,
            RecordingPhase::Review(r) => &r.id,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
pub enum PillEvent {
    Idle,
    Recording { id: String, elapsed_ms: u64 },
    /// Capture stopped; waiting for the user to send or cancel.
    Stopped { id: String, duration_ms: u64 },
    Processing { id: String, label: String, progress: f32 },
    Done { id: String },
    Error { id: Option<String>, message: String },
}

pub fn emit(app: &AppHandle, event: &PillEvent) {
    let _ = app.emit("pill:state", event);
}

pub async fn start(app: AppHandle, state: Arc<AppState>) -> Result<String> {
    {
        let cur = state.current.lock();
        if cur.is_some() {
            return Err(anyhow!("recording already in progress"));
        }
    }

    let id = Uuid::new_v4().to_string();
    let video_path = state.recordings_dir.join(format!("{id}.mp4"));

    cursor::enlarge();

    let capture = capture::start(&state.bin_dir, &video_path).await?;

    let started_at = Instant::now();

    // Persist a "recording" row so the result window can show it immediately.
    let row = Recording {
        id: id.clone(),
        created_at: Utc::now(),
        duration_ms: 0,
        video_path: video_path.to_string_lossy().to_string(),
        status: RecordingStatus::Recording,
        summary: None,
        body: None,
        transcript: None,
        thinking: None,
        error: None,
    };
    state.db().insert_recording(&row).await.ok();

    let auto_stop = {
        let app2 = app.clone();
        let state2 = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(MAX_DURATION_MS)).await;
            if let Err(err) = stop(app2, state2).await {
                tracing::warn!(?err, "auto-stop failed");
            }
        })
    };

    {
        let mut cur = state.current.lock();
        *cur = Some(RecordingPhase::Active(ActiveRecording {
            id: id.clone(),
            video_path: video_path.clone(),
            started_at,
            capture,
            auto_stop_handle: Some(auto_stop),
        }));
    }

    emit(&app, &PillEvent::Recording { id: id.clone(), elapsed_ms: 0 });

    // Tick timer for the pill UI at 10Hz.
    {
        let app2 = app.clone();
        let state2 = state.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                let cur = state2.current.lock();
                let Some(RecordingPhase::Active(active)) = cur.as_ref() else { break };
                if active.id != id2 { break }
                let elapsed = active.started_at.elapsed().as_millis() as u64;
                emit(&app2, &PillEvent::Recording { id: id2.clone(), elapsed_ms: elapsed });
                if elapsed >= MAX_DURATION_MS { break }
            }
        });
    }

    Ok(id)
}

/// Stop capture and enter the "review" phase. The video is on disk but the
/// pipeline does not run until the user explicitly sends.
pub async fn stop(app: AppHandle, state: Arc<AppState>) -> Result<()> {
    let phase = {
        let mut cur = state.current.lock();
        cur.take()
    };
    let mut active = match phase {
        Some(RecordingPhase::Active(a)) => a,
        Some(other) => {
            // Already in review — leave it.
            let mut cur = state.current.lock();
            *cur = Some(other);
            return Ok(());
        }
        None => return Ok(()),
    };

    if let Some(h) = active.auto_stop_handle.take() {
        h.abort();
    }
    cursor::restore();

    let duration_ms = active.started_at.elapsed().as_millis() as u64;
    active.capture.stop().await?;

    // Flip the persisted row from 'recording' → 'stopped' so the result
    // window doesn't keep showing "Recording…" while we wait for the user
    // to send or discard from the pill. Also fill in the captured
    // duration so the sidebar shows it.
    if let Ok(Some(mut rec)) = state.db().get_recording(&active.id).await {
        rec.status = RecordingStatus::Stopped;
        rec.duration_ms = duration_ms;
        let _ = state.db().update_recording(&rec).await;
    }

    {
        let mut cur = state.current.lock();
        *cur = Some(RecordingPhase::Review(ReviewRecording {
            id: active.id.clone(),
            video_path: active.video_path.clone(),
            duration_ms,
        }));
    }

    emit(&app, &PillEvent::Stopped { id: active.id, duration_ms });
    Ok(())
}

/// Confirm send: take the review recording and run the pipeline.
pub async fn send(app: AppHandle, state: Arc<AppState>) -> Result<()> {
    let phase = {
        let mut cur = state.current.lock();
        cur.take()
    };
    let Some(RecordingPhase::Review(review)) = phase else {
        // Reinsert if it was something else (defensive).
        if let Some(other) = phase {
            let mut cur = state.current.lock();
            *cur = Some(other);
        }
        return Err(anyhow!("no recording awaiting send"));
    };

    emit(&app, &PillEvent::Processing { id: review.id.clone(), label: "Preparing video".into(), progress: 0.05 });

    // Open result window so the user sees the streamed instructions.
    if let Some(win) = app.get_webview_window("result") {
        let _ = win.show();
        let _ = win.set_focus();
    }

    let app2 = app.clone();
    let state2 = state.clone();
    let id = review.id.clone();
    let video_path = review.video_path.clone();
    let duration_ms = review.duration_ms;
    tokio::spawn(async move {
        match pipeline::run(app2.clone(), state2.clone(), id.clone(), video_path, duration_ms).await {
            Ok(()) => {
                emit(&app2, &PillEvent::Done { id: id.clone() });
            }
            Err(err) => {
                tracing::error!(?err, recording_id=%id, "pipeline failed");
                let msg = format!("{err:#}");
                if let Ok(Some(mut rec)) = state2.db().get_recording(&id).await {
                    rec.status = RecordingStatus::Failed;
                    rec.error = Some(msg.clone());
                    let _ = state2.db().update_recording(&rec).await;
                }
                emit(&app2, &PillEvent::Error { id: Some(id), message: msg });
            }
        }
    });

    Ok(())
}

pub async fn cancel(app: AppHandle, state: Arc<AppState>) -> Result<()> {
    let phase = {
        let mut cur = state.current.lock();
        cur.take()
    };
    let Some(phase) = phase else { return Ok(()) };

    let (id, video_path) = match phase {
        RecordingPhase::Active(mut active) => {
            if let Some(h) = active.auto_stop_handle.take() { h.abort(); }
            cursor::restore();
            let _ = active.capture.cancel().await;
            (active.id, active.video_path)
        }
        RecordingPhase::Review(review) => (review.id, review.video_path),
    };

    let _ = tokio::fs::remove_file(&video_path).await;
    let _ = state.db().delete_recording(&id).await;

    emit(&app, &PillEvent::Idle);
    Ok(())
}


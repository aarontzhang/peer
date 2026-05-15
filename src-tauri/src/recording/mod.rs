//! Recording lifecycle: drives the Swift sidecar, owns timing, hands off to the pipeline.

mod capture;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::db::{Recording, RecordingStatus};
use crate::pipeline;
use crate::reveal_result_window;
use crate::state::AppState;

pub use capture::CaptureProcess;

// Hard cap as a safety net so a forgotten recording can't fill the disk.
// Display shows elapsed only — no countdown.
const MAX_DURATION_MS: u64 = 10 * 60 * 1000;

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
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum PillEvent {
    Idle,
    Recording {
        id: String,
        elapsed_ms: u64,
    },
    /// Capture stopped; waiting for the user to send or cancel.
    Stopped {
        id: String,
        duration_ms: u64,
    },
    Processing {
        id: String,
        label: String,
        progress: f32,
    },
    Done {
        id: String,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

pub fn emit(app: &AppHandle, event: &PillEvent) {
    let _ = app.emit("pill:state", event);
}

pub async fn start(app: AppHandle, state: Arc<AppState>) -> Result<String> {
    if state.pipeline_in_flight.load(Ordering::Acquire) {
        return Err(anyhow!(
            "another recording is still processing — wait for it to finish before starting a new one"
        ));
    }
    {
        let cur = state.current.lock();
        if cur.is_some() {
            return Err(anyhow!("recording already in progress"));
        }
    }

    let id = Uuid::new_v4().to_string();
    let video_path = state.recordings_dir.join(format!("{id}.mp4"));

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

    emit(
        &app,
        &PillEvent::Recording {
            id: id.clone(),
            elapsed_ms: 0,
        },
    );

    // Tick timer for the pill UI at 10Hz.
    {
        let app2 = app.clone();
        let state2 = state.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
            enum TimerOutcome {
                Continue(u64),
                Failed {
                    id: String,
                    elapsed_ms: u64,
                    message: String,
                },
            }

            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                let outcome = {
                    let mut cur = state2.current.lock();
                    let Some(RecordingPhase::Active(active)) = cur.as_mut() else {
                        break;
                    };
                    if active.id != id2 {
                        break;
                    }
                    let elapsed = active.started_at.elapsed().as_millis() as u64;
                    match active.capture.try_wait() {
                        Ok(Some(status)) => {
                            let id = active.id.clone();
                            if let Some(h) = active.auto_stop_handle.take() {
                                h.abort();
                            }
                            *cur = None;
                            TimerOutcome::Failed {
                                id,
                                elapsed_ms: elapsed,
                                message: format!(
                                    "Capture stopped unexpectedly before you ended the recording: {status}"
                                ),
                            }
                        }
                        Ok(None) => TimerOutcome::Continue(elapsed),
                        Err(err) => {
                            tracing::warn!(?err, "failed to poll capture process");
                            TimerOutcome::Continue(elapsed)
                        }
                    }
                };

                match outcome {
                    TimerOutcome::Continue(elapsed) => {
                        emit(
                            &app2,
                            &PillEvent::Recording {
                                id: id2.clone(),
                                elapsed_ms: elapsed,
                            },
                        );
                        if elapsed >= MAX_DURATION_MS {
                            break;
                        }
                    }
                    TimerOutcome::Failed {
                        id,
                        elapsed_ms,
                        message,
                    } => {
                        if let Ok(Some(mut rec)) = state2.db().get_recording(&id).await {
                            rec.status = RecordingStatus::Failed;
                            rec.duration_ms = elapsed_ms;
                            rec.error = Some(message.clone());
                            let _ = state2.db().update_recording(&rec).await;
                        }
                        emit(
                            &app2,
                            &PillEvent::Error {
                                id: Some(id),
                                message,
                            },
                        );
                        break;
                    }
                }
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

    let duration_ms = active.started_at.elapsed().as_millis() as u64;
    if let Err(err) = active.capture.stop().await {
        let msg = format!("{err:#}");
        if let Ok(Some(mut rec)) = state.db().get_recording(&active.id).await {
            rec.status = RecordingStatus::Failed;
            rec.duration_ms = duration_ms;
            rec.error = Some(msg.clone());
            let _ = state.db().update_recording(&rec).await;
        }
        emit(
            &app,
            &PillEvent::Error {
                id: Some(active.id.clone()),
                message: msg.clone(),
            },
        );
        return Err(anyhow!(msg));
    }

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

    emit(
        &app,
        &PillEvent::Stopped {
            id: active.id,
            duration_ms,
        },
    );
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

    emit(
        &app,
        &PillEvent::Processing {
            id: review.id.clone(),
            label: "Preparing video".into(),
            progress: 0.05,
        },
    );

    // Open result window so the user sees the streamed instructions.
    let _ = reveal_result_window(&app, true);

    let app2 = app.clone();
    let state2 = state.clone();
    let id = review.id.clone();
    let video_path = review.video_path.clone();
    let duration_ms = review.duration_ms;
    state.pipeline_in_flight.store(true, Ordering::Release);
    tokio::spawn(async move {
        let result = pipeline::run(
            app2.clone(),
            state2.clone(),
            id.clone(),
            video_path,
            duration_ms,
        )
        .await;
        state2.pipeline_in_flight.store(false, Ordering::Release);
        match result {
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
                emit(
                    &app2,
                    &PillEvent::Error {
                        id: Some(id),
                        message: msg,
                    },
                );
            }
        }
    });

    Ok(())
}

/// Graceful capture flush for process shutdown (SIGTERM/SIGINT/SIGHUP).
///
/// The Swift sidecar / ffmpeg writes the trailing `moov` atom only when
/// asked to stop cleanly. With `kill_on_drop(true)` on the Child, dropping
/// the handle SIGKILLs the writer and you get a moov-less mp4 that ffprobe
/// rejects. Dev-mode rebuilds in particular SIGTERM the app between
/// recompiles, so without this hook every "save while recording" produces
/// a corrupt file.
///
/// Skips pill events: the app is on its way out and the UI is gone.
pub async fn shutdown(state: Arc<AppState>) {
    let phase = {
        let mut cur = state.current.lock();
        cur.take()
    };
    let Some(RecordingPhase::Active(mut active)) = phase else {
        // Review: file already finalized. None: nothing to flush.
        return;
    };
    if let Some(h) = active.auto_stop_handle.take() {
        h.abort();
    }
    let duration_ms = active.started_at.elapsed().as_millis() as u64;
    if let Err(err) = active.capture.stop().await {
        tracing::warn!(?err, "capture stop failed during shutdown");
    }
    if let Ok(Some(mut rec)) = state.db().get_recording(&active.id).await {
        rec.status = RecordingStatus::Stopped;
        rec.duration_ms = duration_ms;
        let _ = state.db().update_recording(&rec).await;
    }
}

/// Re-run the pipeline on a previously analyzed (or cancelled) recording.
/// The video stays on disk after both cancel and a normal send specifically
/// so the user can re-analyze without having to record again.
pub async fn retry(app: AppHandle, state: Arc<AppState>, id: String) -> Result<()> {
    if state.pipeline_in_flight.load(Ordering::Acquire) {
        return Err(anyhow!(
            "another recording is still processing — wait for it to finish before retrying"
        ));
    }
    {
        let cur = state.current.lock();
        if cur.is_some() {
            return Err(anyhow!(
                "another recording is in progress — stop or send it first"
            ));
        }
    }

    let rec = state
        .db()
        .get_recording(&id)
        .await?
        .ok_or_else(|| anyhow!("recording not found"))?;

    // Retry is allowed from any terminal state (done / failed / canceled).
    // Block only mid-flight statuses, which would race with the live pipeline.
    match rec.status {
        RecordingStatus::Recording | RecordingStatus::Stopped | RecordingStatus::Processing => {
            return Err(anyhow!(
                "this recording is still in progress — wait for it to finish before retrying"
            ));
        }
        _ => {}
    }

    let video_path = PathBuf::from(&rec.video_path);
    if !tokio::fs::try_exists(&video_path).await.unwrap_or(false) {
        // Surface a stable error code so the frontend can disable the retry
        // button with a tooltip instead of throwing a generic alert.
        return Err(anyhow!("VIDEO_MISSING: the original video is no longer on disk"));
    }

    let duration_ms = rec.duration_ms;

    emit(
        &app,
        &PillEvent::Processing {
            id: id.clone(),
            label: "Preparing video".into(),
            progress: 0.05,
        },
    );

    let _ = reveal_result_window(&app, true);

    let app2 = app.clone();
    let state2 = state.clone();
    let id2 = id.clone();
    state.pipeline_in_flight.store(true, Ordering::Release);
    tokio::spawn(async move {
        let result = pipeline::run(
            app2.clone(),
            state2.clone(),
            id2.clone(),
            video_path,
            duration_ms,
        )
        .await;
        state2.pipeline_in_flight.store(false, Ordering::Release);
        match result {
            Ok(()) => {
                emit(&app2, &PillEvent::Done { id: id2.clone() });
            }
            Err(err) => {
                tracing::error!(?err, recording_id=%id2, "retry pipeline failed");
                let msg = format!("{err:#}");
                if let Ok(Some(mut r)) = state2.db().get_recording(&id2).await {
                    r.status = RecordingStatus::Failed;
                    r.error = Some(msg.clone());
                    let _ = state2.db().update_recording(&r).await;
                }
                emit(
                    &app2,
                    &PillEvent::Error {
                        id: Some(id2),
                        message: msg,
                    },
                );
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

    // Active vs Review cancellations behave differently:
    //   - Active: capture was still running; nothing the user could "send"
    //     existed yet, so drop the row entirely.
    //   - Review: capture finished and the user explicitly chose to discard.
    //     Keep the row (status = canceled) AND the video file on disk so the
    //     user can change their mind from the result window and re-analyze.
    //     The video is only purged when the user explicitly deletes the row.
    match phase {
        RecordingPhase::Active(mut active) => {
            if let Some(h) = active.auto_stop_handle.take() {
                h.abort();
            }
            let _ = active.capture.cancel().await;
            let _ = tokio::fs::remove_file(&active.video_path).await;
            let _ = state.db().delete_recording(&active.id).await;
        }
        RecordingPhase::Review(review) => {
            if let Ok(Some(mut rec)) = state.db().get_recording(&review.id).await {
                rec.status = RecordingStatus::Canceled;
                let _ = state.db().update_recording(&rec).await;
            }
        }
    };

    emit(&app, &PillEvent::Idle);
    Ok(())
}

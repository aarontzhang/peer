//! Recording lifecycle: drives the Swift sidecar, owns timing, hands off to the pipeline.

mod capture;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
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

/// Bring in a pre-recorded video the user picked from disk and run the
/// regular pipeline against it. Mirrors `retry` once the file is staged.
///
/// The entry gate is an atomic claim on `pipeline_in_flight`. Without
/// that, two concurrent uploads (or upload + retry) could both pass a
/// `load()` check while one is ffprobing/copying gigabytes; the claim
/// closes that window and is released on any early failure.
pub async fn upload(
    app: AppHandle,
    state: Arc<AppState>,
    source_path: String,
) -> Result<String> {
    // Atomic claim — fails fast if anything else owns the worker slot.
    if state
        .pipeline_in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(anyhow!(
            "another recording is still processing — wait for it to finish before uploading"
        ));
    }
    // From here on, any early return MUST release the claim. The pipeline
    // spawn at the end transfers ownership of the flag to the spawned task.
    let release_on_error = ReleaseFlag::new(state.pipeline_in_flight.clone());

    {
        let cur = state.current.lock();
        if cur.is_some() {
            return Err(anyhow!(
                "a recording is in progress — stop or send it first"
            ));
        }
    }
    if !crate::saas::account_status().signed_in {
        return Err(anyhow!("Sign in to use Peer — uploads require a Peer account"));
    }

    let source = PathBuf::from(&source_path);
    if !tokio::fs::try_exists(&source).await.unwrap_or(false) {
        return Err(anyhow!("the selected file no longer exists"));
    }

    // Probe up-front so a non-video file is rejected before we copy
    // gigabytes. ffprobe also doubles as a container sniff — we don't
    // trust the filename extension on its own.
    let probe = crate::pipeline::ffprobe::probe(&source)
        .await
        .map_err(|e| anyhow!("couldn't read that file as a video: {e}"))?;
    if probe.duration_secs <= 0.0 {
        return Err(anyhow!(
            "ffprobe reported zero-length video — pick a different file"
        ));
    }
    let duration_ms = (probe.duration_secs * 1000.0).round().max(0.0) as u64;

    let id = Uuid::new_v4().to_string();
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "mp4".to_string());
    let final_path = state.recordings_dir.join(format!("{id}.{ext}"));
    let staging_path = state.recordings_dir.join(format!("{id}.{ext}.partial"));

    // Row first, then copy → rename. The DB init sweep already handles
    // 'processing' rows with no body by deleting them, so a crash here
    // leaves a clean-ish state on next launch; the matching .partial
    // sweep in `init_sweep_partials` mops up the orphan blob.
    let row = Recording {
        id: id.clone(),
        created_at: Utc::now(),
        duration_ms,
        video_path: final_path.to_string_lossy().to_string(),
        status: RecordingStatus::Processing,
        summary: None,
        body: None,
        transcript: None,
        thinking: None,
        error: None,
    };
    state
        .db()
        .insert_recording(&row)
        .await
        .with_context(|| "registering uploaded recording in the database")?;

    emit(
        &app,
        &PillEvent::Processing {
            id: id.clone(),
            label: "Copying video".into(),
            progress: 0.02,
        },
    );

    if let Err(err) = tokio::fs::copy(&source, &staging_path).await {
        let _ = tokio::fs::remove_file(&staging_path).await;
        fail_row(&state, &id, &format!("copying selected video: {err}")).await;
        return Err(anyhow!("copying selected video: {err}"));
    }

    if let Err(err) = tokio::fs::rename(&staging_path, &final_path).await {
        let _ = tokio::fs::remove_file(&staging_path).await;
        fail_row(&state, &id, &format!("finalizing copy: {err}")).await;
        return Err(anyhow!("finalizing copy: {err}"));
    }

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
    let target2 = final_path.clone();
    // The spawned pipeline owns the flag from here on.
    release_on_error.defuse();
    tokio::spawn(async move {
        let result = pipeline::run(
            app2.clone(),
            state2.clone(),
            id2.clone(),
            target2,
            duration_ms,
        )
        .await;
        state2.pipeline_in_flight.store(false, Ordering::Release);
        match result {
            Ok(()) => {
                emit(&app2, &PillEvent::Done { id: id2.clone() });
            }
            Err(err) => {
                tracing::error!(?err, recording_id=%id2, "upload pipeline failed");
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

    Ok(id)
}

async fn fail_row(state: &Arc<AppState>, id: &str, message: &str) {
    if let Ok(Some(mut rec)) = state.db().get_recording(id).await {
        rec.status = RecordingStatus::Failed;
        rec.error = Some(message.to_string());
        let _ = state.db().update_recording(&rec).await;
    }
}

/// RAII guard for `pipeline_in_flight`. Set the flag with
/// `compare_exchange`, hand the guard to the stack frame; any early
/// return clears the flag on drop. `defuse()` transfers ownership to
/// the spawned pipeline task.
struct ReleaseFlag {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl ReleaseFlag {
    fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag, armed: true }
    }
    fn defuse(mut self) {
        self.armed = false;
    }
}

impl Drop for ReleaseFlag {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::Release);
        }
    }
}

/// Best-effort sweep of `*.partial` files in the recordings dir, called
/// once on startup. A crash mid-upload-copy leaves these orphans; they
/// have no DB row pointing to them and aren't useful for retry.
pub async fn sweep_partial_uploads(state: Arc<AppState>) {
    let mut entries = match tokio::fs::read_dir(&state.recordings_dir).await {
        Ok(e) => e,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("partial") {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
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

//! Global hotkeys for toggling recording.
//!
//! Two parallel paths share a single toggle channel:
//!
//! - **Fn tap** (macOS only): a clean tap of the Fn/Globe key. Implemented
//!   via a low-level CGEventTap watching `kCGEventFlagsChanged`. Requires
//!   Accessibility permission; the dev-build path frequently loses TCC
//!   approval after enough rebuilds, which is why we have a backup.
//! - **Cmd+Shift+R chord**: registered through `tauri-plugin-global-shortcut`,
//!   which uses `RegisterEventHotKey` under the hood. Doesn't need
//!   Accessibility — works in dev even when TCC is being weird.
//!
//! Both paths feed the same `mpsc` toggle channel; a single tokio task
//! reads from it and decides start vs. stop based on current recording
//! state, so the two hotkeys can't get out of sync.

#[cfg(target_os = "macos")]
mod fn_tap;
mod global_chord;

use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::recording::{self, RecordingPhase};
use crate::state::AppState;

pub fn install(app: AppHandle, state: Arc<AppState>) {
    let (tx, rx) = mpsc::unbounded_channel::<()>();

    spawn_toggle_consumer(app.clone(), state.clone(), rx);

    #[cfg(target_os = "macos")]
    fn_tap::install(app.clone(), state.clone(), tx.clone());

    global_chord::install(app, state, tx);
}

/// One consumer of toggle signals — owns the AppHandle and reads the
/// current recording phase to decide start vs. stop. Both the Fn tap and
/// the chord push into the same channel, so the two can't race.
fn spawn_toggle_consumer(
    app: AppHandle,
    state: Arc<AppState>,
    mut rx: mpsc::UnboundedReceiver<()>,
) {
    tauri::async_runtime::spawn(async move {
        while rx.recv().await.is_some() {
            let phase_kind = {
                let cur = state.current.lock();
                match &*cur {
                    Some(RecordingPhase::Active(_)) => Phase::Active,
                    Some(RecordingPhase::Review(_)) => Phase::Review,
                    None => Phase::Idle,
                }
            };
            let res = match phase_kind {
                Phase::Active => recording::stop(app.clone(), state.clone()).await,
                Phase::Idle => recording::start(app.clone(), state.clone())
                    .await
                    .map(|_| ()),
                // Awaiting send/cancel — leave the review state alone.
                Phase::Review => Ok(()),
            };
            if let Err(err) = res {
                tracing::warn!(?err, "hotkey toggle failed");
            }
        }
    });
}

enum Phase {
    Idle,
    Active,
    Review,
}

/// Snapshot of whether the global Fn-tap hotkey is wired up. Surfaces to
/// the UI so the user can see if Accessibility access is missing rather
/// than silently wondering why the key does nothing.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    /// `true` once the CGEventTap is created and listening.
    pub installed: bool,
    /// Human-readable reason when `installed` is false.
    pub reason: Option<String>,
}

impl HotkeyStatus {
    pub fn unknown() -> Self {
        Self {
            installed: false,
            reason: Some(
                "Hotkey is initializing. If this persists, grant Peer \
                 Accessibility access in System Settings → Privacy & Security \
                 → Accessibility."
                    .into(),
            ),
        }
    }
}

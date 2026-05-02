//! Global hotkey: a clean tap of the **Fn** (Globe) key toggles recording.
//!
//! Implementation notes:
//! - macOS only. We watch `kCGEventFlagsChanged` via a CGEventTap and treat
//!   an Fn-down → Fn-up cycle shorter than `TAP_THRESHOLD` with no other key
//!   pressed in between as a "tap". Holding Fn while pressing F1/brightness/
//!   etc. is intentionally ignored — that's the OS's normal Fn-modifier use.
//! - Requires Accessibility permission. Without it, `CGEventTap::with_enabled`
//!   returns `Err(())`; we log a warning and silently disable the hotkey.
//!   The user can grant access in System Settings → Privacy & Security →
//!   Accessibility, then restart the app.
//! - The tap runs on its own dedicated thread driving a CFRunLoop. Toggle
//!   signals are sent across an unbounded channel to a tokio task that owns
//!   the AppHandle and decides start vs. stop based on current recording
//!   state.

#[cfg(target_os = "macos")]
mod fn_tap;

use std::sync::Arc;

use tauri::AppHandle;

use crate::state::AppState;

pub fn install(app: AppHandle, state: Arc<AppState>) {
    #[cfg(target_os = "macos")]
    fn_tap::install(app, state);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
    }
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
                "Hotkey is initializing. If this persists, grant Hummingbird \
                 Accessibility access in System Settings → Privacy & Security \
                 → Accessibility."
                    .into(),
            ),
        }
    }
}

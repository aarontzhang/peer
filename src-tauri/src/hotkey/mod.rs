//! Global hotkeys for toggling recording.
//!
//! The selected recording keybind is persisted locally and feeds one shared
//! toggle channel. Modifier-only taps use a CGEventTap; Cmd+Shift+R uses
//! `tauri-plugin-global-shortcut`.

#[cfg(target_os = "macos")]
mod fn_tap;
mod global_chord;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
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

pub fn set_recording_keybind(
    app: AppHandle,
    state: Arc<AppState>,
    keybind: RecordingKeybind,
) -> Result<HotkeyStatus> {
    save_recording_keybind(&state.data_dir, &keybind)?;
    {
        let mut selected = state.recording_keybind.lock();
        *selected = keybind;
    }
    global_chord::sync_registration(&app, &state);
    Ok(publish_status(&app, &state))
}

/// One consumer of toggle signals owns the AppHandle and reads the current
/// recording phase to decide start vs. stop.
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
                // Awaiting send/cancel: leave the review state alone.
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

/// User-selected recording trigger. `Fn` and `RightOption` use a CGEventTap
/// to detect a tap of the modifier alone; `Chord` registers an arbitrary
/// global accelerator via `tauri-plugin-global-shortcut`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RecordingKeybind {
    Fn,
    RightOption,
    Chord {
        mods: Vec<String>,
        code: String,
        label: String,
    },
}

impl Default for RecordingKeybind {
    fn default() -> Self {
        Self::Fn
    }
}

impl RecordingKeybind {
    pub fn label(&self) -> String {
        match self {
            Self::Fn => "Fn".into(),
            Self::RightOption => "Right Option".into(),
            Self::Chord { label, .. } => label.clone(),
        }
    }

    pub fn is_modifier_tap(&self) -> bool {
        matches!(self, Self::Fn | Self::RightOption)
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    #[serde(default)]
    recording_keybind: RecordingKeybind,
}

pub fn load_recording_keybind(data_dir: &Path) -> RecordingKeybind {
    let path = data_dir.join("settings.json");
    let Ok(bytes) = std::fs::read(path) else {
        return RecordingKeybind::default();
    };
    serde_json::from_slice::<SettingsFile>(&bytes)
        .map(|s| s.recording_keybind)
        .unwrap_or_default()
}

fn save_recording_keybind(data_dir: &Path, keybind: &RecordingKeybind) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("settings.json");
    let bytes = serde_json::to_vec_pretty(&SettingsFile {
        recording_keybind: keybind.clone(),
    })
    .context("serialize hotkey settings")?;
    std::fs::write(path, bytes).context("write hotkey settings")
}

#[derive(Debug, Clone, Default)]
pub struct HotkeyAvailability {
    pub modifier_tap: Option<std::result::Result<(), String>>,
    pub chord: Option<std::result::Result<(), String>>,
}

pub fn set_modifier_tap_availability(
    app: &AppHandle,
    state: &AppState,
    availability: std::result::Result<(), String>,
) {
    {
        let mut a = state.hotkey_availability.lock();
        a.modifier_tap = Some(availability);
    }
    publish_status(app, state);
}

pub fn set_chord_availability(
    app: &AppHandle,
    state: &AppState,
    availability: std::result::Result<(), String>,
) {
    {
        let mut a = state.hotkey_availability.lock();
        a.chord = Some(availability);
    }
    publish_status(app, state);
}

pub fn publish_status(app: &AppHandle, state: &AppState) -> HotkeyStatus {
    let keybind = state.recording_keybind.lock().clone();
    let status = status_for(&keybind, &state.hotkey_availability.lock());
    {
        let mut s = state.hotkey_status.lock();
        *s = status.clone();
    }
    let _ = app.emit("hotkey:status", &status);
    status
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub keybind: RecordingKeybind,
    pub label: String,
    /// `true` once the selected recording trigger is created and listening.
    pub installed: bool,
    /// Human-readable reason when `installed` is false.
    pub reason: Option<String>,
}

impl HotkeyStatus {
    pub fn unknown(keybind: RecordingKeybind) -> Self {
        let label = keybind.label();
        Self {
            keybind,
            label,
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

fn status_for(keybind: &RecordingKeybind, availability: &HotkeyAvailability) -> HotkeyStatus {
    let selected = if keybind.is_modifier_tap() {
        &availability.modifier_tap
    } else {
        &availability.chord
    };
    let label = keybind.label();
    match selected {
        Some(Ok(())) => HotkeyStatus {
            keybind: keybind.clone(),
            label,
            installed: true,
            reason: None,
        },
        Some(Err(reason)) => HotkeyStatus {
            keybind: keybind.clone(),
            label,
            installed: false,
            reason: Some(reason.clone()),
        },
        None => HotkeyStatus::unknown(keybind.clone()),
    }
}

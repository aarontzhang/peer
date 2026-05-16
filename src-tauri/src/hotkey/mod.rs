//! Global hotkeys for toggling recording.
//!
//! The selected recording keybind is persisted locally and feeds one shared
//! toggle channel. Modifier-only taps (Right Option, Fn) use a CGEventTap;
//! arbitrary chords go through `tauri-plugin-global-shortcut`.

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
        Self::RightOption
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

/// Permission mode for the generated agent prompt. `Ask` makes the prompt
/// tell the downstream agent to confirm with the user at critical steps;
/// `Bypass` tells it to run end-to-end without check-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Ask,
    Bypass,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Ask
    }
}

impl PermissionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Bypass => "bypass",
        }
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    #[serde(default)]
    recording_keybind: RecordingKeybind,
    #[serde(default)]
    mode: PermissionMode,
}

fn load_settings(data_dir: &Path) -> SettingsFile {
    let path = data_dir.join("settings.json");
    let Ok(bytes) = std::fs::read(path) else {
        return SettingsFile::default();
    };
    serde_json::from_slice::<SettingsFile>(&bytes).unwrap_or_default()
}

fn save_settings(data_dir: &Path, settings: &SettingsFile) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("settings.json");
    let bytes = serde_json::to_vec_pretty(settings).context("serialize settings")?;
    std::fs::write(path, bytes).context("write settings")
}

pub fn load_recording_keybind(data_dir: &Path) -> RecordingKeybind {
    load_settings(data_dir).recording_keybind
}

pub fn load_permission_mode(data_dir: &Path) -> PermissionMode {
    load_settings(data_dir).mode
}

fn save_recording_keybind(data_dir: &Path, keybind: &RecordingKeybind) -> Result<()> {
    let mut current = load_settings(data_dir);
    current.recording_keybind = keybind.clone();
    save_settings(data_dir, &current)
}

fn save_permission_mode(data_dir: &Path, mode: PermissionMode) -> Result<()> {
    let mut current = load_settings(data_dir);
    current.mode = mode;
    save_settings(data_dir, &current)
}

pub fn set_permission_mode(state: Arc<AppState>, mode: PermissionMode) -> Result<PermissionMode> {
    save_permission_mode(&state.data_dir, mode)?;
    {
        let mut selected = state.permission_mode.lock();
        *selected = mode;
    }
    Ok(mode)
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

#[cfg(test)]
mod tests {
    use super::{
        load_permission_mode, load_recording_keybind, save_permission_mode, save_recording_keybind,
        status_for, HotkeyAvailability, PermissionMode, RecordingKeybind,
    };
    use tempfile::tempdir;

    #[test]
    fn settings_default_to_right_option_and_ask_mode() {
        let dir = tempdir().unwrap();

        assert_eq!(
            load_recording_keybind(dir.path()),
            RecordingKeybind::RightOption
        );
        assert_eq!(load_permission_mode(dir.path()), PermissionMode::Ask);
    }

    #[test]
    fn settings_round_trip_keybind_and_permission_mode() {
        let dir = tempdir().unwrap();
        let keybind = RecordingKeybind::Chord {
            mods: vec!["super".into(), "shift".into()],
            code: "KeyK".into(),
            label: "⌘+⇧+K".into(),
        };

        save_recording_keybind(dir.path(), &keybind).unwrap();
        save_permission_mode(dir.path(), PermissionMode::Bypass).unwrap();

        assert_eq!(load_recording_keybind(dir.path()), keybind);
        assert_eq!(load_permission_mode(dir.path()), PermissionMode::Bypass);
    }

    #[test]
    fn malformed_settings_fall_back_to_defaults() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();

        assert_eq!(
            load_recording_keybind(dir.path()),
            RecordingKeybind::RightOption
        );
        assert_eq!(load_permission_mode(dir.path()), PermissionMode::Ask);
    }

    #[test]
    fn status_uses_selected_hotkey_backend() {
        let availability = HotkeyAvailability {
            modifier_tap: Some(Err("Accessibility denied".into())),
            chord: Some(Ok(())),
        };

        let right_option = status_for(&RecordingKeybind::RightOption, &availability);
        assert!(!right_option.installed);
        assert_eq!(right_option.reason.as_deref(), Some("Accessibility denied"));

        let chord = status_for(
            &RecordingKeybind::Chord {
                mods: vec!["super".into()],
                code: "KeyK".into(),
                label: "⌘+K".into(),
            },
            &availability,
        );
        assert!(chord.installed);
        assert_eq!(chord.reason, None);
    }
}

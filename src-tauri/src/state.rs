use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::db::Db;
use crate::hotkey::{self, HotkeyAvailability, HotkeyStatus, PermissionMode, RecordingKeybind};
use crate::recording::RecordingController;

/// Process-wide state managed by Tauri. Cheap to clone (`Arc` internals).
pub struct AppState {
    db: Db,
    recording: RecordingController,
    pub data_dir: PathBuf,
    pub recordings_dir: PathBuf,
    pub frames_dir: PathBuf,
    pub bin_dir: PathBuf,
    /// Set while a recording is in flight; clears the moment we hand the file
    /// to the pipeline.
    pub current: Arc<Mutex<Option<crate::recording::RecordingPhase>>>,
    /// User-selected recording keybind, persisted in app data.
    pub recording_keybind: Arc<Mutex<RecordingKeybind>>,
    /// Permission mode that shapes the generated agent prompt.
    pub permission_mode: Arc<Mutex<PermissionMode>>,
    /// Backend availability for each hotkey mechanism.
    pub hotkey_availability: Arc<Mutex<HotkeyAvailability>>,
    /// Live status of the selected recording hotkey. Updated from the hotkey
    /// module once setup either succeeds or fails.
    pub hotkey_status: Arc<Mutex<HotkeyStatus>>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> anyhow::Result<Self> {
        let data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")));
        let recordings_dir = data_dir.join("recordings");
        let frames_dir = data_dir.join("frames");
        let bin_dir = app
            .path()
            .resource_dir()
            .ok()
            .map(|p| p.join("bin"))
            .unwrap_or_else(|| data_dir.join("bin"));

        std::fs::create_dir_all(&recordings_dir)?;
        std::fs::create_dir_all(&frames_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        let recording_keybind = hotkey::load_recording_keybind(&data_dir);
        let permission_mode = hotkey::load_permission_mode(&data_dir);

        Ok(Self {
            db: Db::new(data_dir.join("peer.db")),
            recording: RecordingController::default(),
            data_dir,
            recordings_dir,
            frames_dir,
            bin_dir,
            current: Arc::new(Mutex::new(None)),
            recording_keybind: Arc::new(Mutex::new(recording_keybind.clone())),
            permission_mode: Arc::new(Mutex::new(permission_mode)),
            hotkey_availability: Arc::new(Mutex::new(HotkeyAvailability::default())),
            hotkey_status: Arc::new(Mutex::new(HotkeyStatus::unknown(recording_keybind))),
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
    pub fn recording(&self) -> &RecordingController {
        &self.recording
    }
}

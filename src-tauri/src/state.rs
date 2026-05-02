use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::db::Db;
use crate::hotkey::HotkeyStatus;
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
    /// Live status of the global Fn-tap. Updated from the hotkey module
    /// once the CGEventTap setup either succeeds or fails. The UI reads
    /// this via `get_hotkey_status` to surface a banner when the tap
    /// couldn't be installed (typically: missing Accessibility access).
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

        Ok(Self {
            db: Db::new(data_dir.join("hummingbird.db")),
            recording: RecordingController::default(),
            data_dir,
            recordings_dir,
            frames_dir,
            bin_dir,
            current: Arc::new(Mutex::new(None)),
            hotkey_status: Arc::new(Mutex::new(HotkeyStatus::unknown())),
        })
    }

    pub fn db(&self) -> &Db { &self.db }
    pub fn recording(&self) -> &RecordingController { &self.recording }
}

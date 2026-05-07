//! Cmd+Shift+R global hotkey, registered via `tauri-plugin-global-shortcut`.

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
use tokio::sync::mpsc;

use crate::hotkey::{self, RecordingKeybind};
use crate::state::AppState;

fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyR)
}

pub fn install(app: AppHandle, state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    if app
        .try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
        .is_none()
    {
        let plugin_tx = tx.clone();
        let plugin_state = state.clone();
        let toggle_for_handler = toggle_shortcut();
        let plugin = tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |_app, shortcut, event| {
                let selected = *plugin_state.recording_keybind.lock();
                if selected == RecordingKeybind::CmdShiftR
                    && event.state() == ShortcutState::Pressed
                    && shortcut == &toggle_for_handler
                {
                    let _ = plugin_tx.send(());
                }
            })
            .build();
        if let Err(err) = app.plugin(plugin) {
            let reason = format!("Could not install the global shortcut plugin: {err}");
            tracing::warn!(?err, "failed to register global-shortcut plugin");
            hotkey::set_cmd_shift_r_availability(&app, &state, Err(reason));
            return;
        }
    }

    sync_registration(&app, &state);
}

pub fn sync_registration(app: &AppHandle, state: &AppState) {
    let selected = *state.recording_keybind.lock();
    let shortcut = toggle_shortcut();
    let Some(global_shortcut) =
        app.try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
    else {
        hotkey::set_cmd_shift_r_availability(
            app,
            state,
            Err("The global shortcut plugin is unavailable.".into()),
        );
        return;
    };

    if selected == RecordingKeybind::CmdShiftR {
        if global_shortcut.is_registered(shortcut.clone()) {
            hotkey::set_cmd_shift_r_availability(app, state, Ok(()));
            return;
        }

        match global_shortcut.register(shortcut) {
            Ok(()) => {
                eprintln!("[peer] Cmd+Shift+R hotkey installed.");
                hotkey::set_cmd_shift_r_availability(app, state, Ok(()));
            }
            Err(err) => {
                let reason = format!("Cmd+Shift+R could not be registered: {err}");
                tracing::warn!(?err, "failed to register Cmd+Shift+R");
                eprintln!("[peer] Cmd+Shift+R hotkey UNAVAILABLE: {err}");
                hotkey::set_cmd_shift_r_availability(app, state, Err(reason));
            }
        }
    } else {
        if global_shortcut.is_registered(shortcut.clone()) {
            if let Err(err) = global_shortcut.unregister(shortcut) {
                tracing::warn!(?err, "failed to unregister Cmd+Shift+R");
            }
        }
        hotkey::set_cmd_shift_r_availability(app, state, Ok(()));
    }
}

//! Cmd+Shift+R global hotkey, registered via `tauri-plugin-global-shortcut`.
//!
//! Backup for the Fn-tap path. Uses `RegisterEventHotKey` under the hood,
//! which doesn't need Accessibility permission — so it survives the dev-
//! build TCC churn that frequently kills the Fn tap.

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio::sync::mpsc;

use crate::state::AppState;

fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyR)
}

pub fn install(app: AppHandle, state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    let toggle = toggle_shortcut();

    // The plugin needs to be registered before we can call into it. We
    // do it here so the hotkey module owns its dependency end-to-end.
    if app
        .try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
        .is_none()
    {
        let plugin_tx = tx.clone();
        let toggle_for_handler = toggle.clone();
        let plugin = tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |_app, shortcut, event| {
                // Fire on key-down only — `GlobalShortcut` reports both press
                // and release; we only want one toggle per chord.
                if event.state() == ShortcutState::Pressed && shortcut == &toggle_for_handler {
                    let _ = plugin_tx.send(());
                }
            })
            .build();
        if let Err(err) = app.plugin(plugin) {
            tracing::warn!(?err, "failed to register global-shortcut plugin");
            let _ = state;
            return;
        }
    }

    match app.global_shortcut().register(toggle) {
        Ok(()) => {
            eprintln!("[peer] Cmd+Shift+R hotkey installed.");
        }
        Err(err) => {
            tracing::warn!(?err, "failed to register Cmd+Shift+R");
            eprintln!("[peer] Cmd+Shift+R hotkey UNAVAILABLE: {err}");
        }
    }

    let _ = state;
}

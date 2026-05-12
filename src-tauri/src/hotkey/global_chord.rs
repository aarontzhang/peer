//! User-defined chord global hotkey, registered via
//! `tauri-plugin-global-shortcut`. The chord is rebuilt from the saved
//! `RecordingKeybind::Chord` shape on every sync.

use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
use tokio::sync::mpsc;

use crate::hotkey::{self, RecordingKeybind};
use crate::state::AppState;

/// Tracks the chord we're currently registered for so we can unregister it
/// on change. Held inside the plugin handler closure.
type RegisteredShortcut = Arc<Mutex<Option<Shortcut>>>;

pub fn install(app: AppHandle, state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    if app
        .try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
        .is_none()
    {
        let plugin_tx = tx.clone();
        let plugin_state = state.clone();
        let plugin = tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |_app, shortcut, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                let selected = plugin_state.recording_keybind.lock().clone();
                let Some(ChordResult::Shortcut(target)) = chord_from_keybind(&selected) else {
                    return;
                };
                if shortcut == &target {
                    let _ = plugin_tx.send(());
                }
            })
            .build();
        if let Err(err) = app.plugin(plugin) {
            let reason = format!("Could not install the global shortcut plugin: {err}");
            tracing::warn!(?err, "failed to register global-shortcut plugin");
            hotkey::set_chord_availability(&app, &state, Err(reason));
            return;
        }
    }

    sync_registration(&app, &state);
}

pub fn sync_registration(app: &AppHandle, state: &AppState) {
    let selected = state.recording_keybind.lock().clone();
    let Some(global_shortcut) =
        app.try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
    else {
        hotkey::set_chord_availability(
            app,
            state,
            Err("The global shortcut plugin is unavailable.".into()),
        );
        return;
    };

    // Track the previously registered chord across calls so we can unregister
    // it when the user picks a new one.
    static PREV: OnceLock<RegisteredShortcut> = OnceLock::new();
    let mut prev = PREV
        .get_or_init(|| Arc::new(Mutex::new(None)))
        .lock()
        .unwrap();
    if let Some(old) = prev.take() {
        if global_shortcut.is_registered(old.clone()) {
            if let Err(err) = global_shortcut.unregister(old) {
                tracing::warn!(?err, "failed to unregister previous chord");
            }
        }
    }

    match chord_from_keybind(&selected) {
        Some(ChordResult::Shortcut(shortcut)) => match global_shortcut.register(shortcut.clone()) {
            Ok(()) => {
                eprintln!("[peer] Chord hotkey installed: {}", selected.label());
                *prev = Some(shortcut);
                hotkey::set_chord_availability(app, state, Ok(()));
            }
            Err(err) => {
                let reason = format!("{} could not be registered: {err}", selected.label());
                tracing::warn!(?err, "failed to register chord");
                eprintln!("[peer] Chord hotkey UNAVAILABLE: {err}");
                hotkey::set_chord_availability(app, state, Err(reason));
            }
        },
        Some(ChordResult::Rejected(reason)) => {
            tracing::warn!(reason = %reason, "refused to register chord");
            eprintln!("[peer] Chord hotkey UNAVAILABLE: {reason}");
            hotkey::set_chord_availability(app, state, Err(reason));
        }
        None => {
            // Fn / Right Option taps don't use the global-shortcut path.
            hotkey::set_chord_availability(app, state, Ok(()));
        }
    }
}

enum ChordResult {
    Shortcut(Shortcut),
    Rejected(String),
}

fn chord_from_keybind(keybind: &RecordingKeybind) -> Option<ChordResult> {
    let RecordingKeybind::Chord { mods, code, .. } = keybind else {
        return None;
    };
    let mut modifiers = Modifiers::empty();
    for m in mods {
        match m.as_str() {
            "super" | "cmd" | "meta" => modifiers |= Modifiers::SUPER,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            other => {
                tracing::warn!(modifier = other, "unknown chord modifier");
                return Some(ChordResult::Rejected(format!(
                    "Can't use “{other}” as a modifier."
                )));
            }
        }
    }
    let Some(parsed_code) = Code::from_str(code).ok() else {
        return Some(ChordResult::Rejected(format!(
            "Can't use “{code}” as a shortcut key.",
        )));
    };
    if modifiers.is_empty() {
        return Some(ChordResult::Rejected(
            "Add a modifier (⌘, ⌃, ⌥, or ⇧) to use this shortcut.".into(),
        ));
    }
    Some(ChordResult::Shortcut(Shortcut::new(Some(modifiers), parsed_code)))
}

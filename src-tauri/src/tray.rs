//! macOS menu-bar (status item) for Peer.
//!
//! Adds a system-tray icon next to the clock with a small menu mirroring the
//! pill's primary actions: open the result window, start/stop a recording,
//! show/hide the pill, and quit. The "Record" item's label flips between
//! "Start Recording" and "Stop Recording" in response to `pill:state` events,
//! so it stays honest about which action a click will perform.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Deserialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Listener, Manager, Wry};

use crate::state::AppState;
use crate::{recording, reveal_result_window};

const ID_OPEN: &str = "tray.open";
const ID_RECORD: &str = "tray.record";
const ID_TOGGLE_PILL: &str = "tray.toggle_pill";
const ID_QUIT: &str = "tray.quit";

const LABEL_START_RECORDING: &str = "Start Recording";
const LABEL_STOP_RECORDING: &str = "Stop Recording";
const LABEL_HIDE_PILL: &str = "Hide Pill";
const LABEL_SHOW_PILL: &str = "Show Pill";

/// Minimal shape of a `pill:state` payload. The recording lifecycle emits the
/// full enum (see `PillEvent` in `recording/mod.rs`); we only need the tag.
#[derive(Deserialize)]
struct PillStateProbe {
    kind: String,
}

/// Install the menu-bar status item. Safe to call once during setup.
pub fn install(app: &AppHandle, state: Arc<AppState>) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, ID_OPEN, "Open Peer", true, None::<&str>)?;
    let record_item = MenuItem::with_id(app, ID_RECORD, LABEL_START_RECORDING, true, None::<&str>)?;
    let pill_item = MenuItem::with_id(app, ID_TOGGLE_PILL, LABEL_HIDE_PILL, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, ID_QUIT, "Quit Peer", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&open_item, &record_item, &pill_item, &separator, &quit_item],
    )?;

    let record_handle: Arc<MenuItem<Wry>> = Arc::new(record_item);
    let pill_handle: Arc<MenuItem<Wry>> = Arc::new(pill_item);

    let mut builder = TrayIconBuilder::with_id("peer-tray")
        .menu(&menu)
        .tooltip("Peer")
        .show_menu_on_left_click(true)
        .on_menu_event({
            let state = state.clone();
            let pill_handle = pill_handle.clone();
            move |app, event| {
                handle_menu_event(app, &state, &pill_handle, event.id().as_ref());
            }
        });

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    if let Some(image) = load_tray_image() {
        builder = builder.icon(image);
    } else if let Some(fallback) = app.default_window_icon().cloned() {
        // Older builds without the bundled template asset fall back to the
        // app icon. It won't theme as nicely on the menu bar but is correct.
        builder = builder.icon(fallback);
    }

    let _tray = builder.build(app)?;

    install_state_listener(app, record_handle);

    Ok(())
}

/// Pure-black, alpha-AA glasses mark, generated from the same geometry as the
/// pill SVG (see `scripts/`). macOS recolors template images to match the
/// menu-bar appearance.
fn load_tray_image() -> Option<tauri::image::Image<'static>> {
    const TRAY_2X: &[u8] = include_bytes!("../icons/tray-icon@2x.png");
    tauri::image::Image::from_bytes(TRAY_2X).ok()
}

fn handle_menu_event(
    app: &AppHandle,
    state: &Arc<AppState>,
    pill_handle: &Arc<MenuItem<Wry>>,
    id: &str,
) {
    match id {
        ID_OPEN => {
            if let Err(err) = reveal_result_window(app, true) {
                tracing::warn!(?err, "tray Open failed");
            }
        }
        ID_RECORD => {
            // `current` holds both Active and Review phases — for tray
            // purposes either counts as "in-flight". `stop` is the right
            // action while actively recording; in Review the user should use
            // the pill's send/cancel buttons instead, but calling stop again
            // is a harmless no-op.
            let active = state.current.lock().is_some();
            let app2 = app.clone();
            let state2 = state.clone();
            tauri::async_runtime::spawn(async move {
                let result = if active {
                    recording::stop(app2, state2).await
                } else {
                    recording::start(app2, state2).await.map(|_| ())
                };
                if let Err(err) = result {
                    tracing::warn!(?err, "tray recording toggle failed");
                }
            });
        }
        ID_TOGGLE_PILL => {
            if let Some(win) = app.get_webview_window("pill") {
                let visible = win.is_visible().unwrap_or(false);
                let res = if visible { win.hide() } else { win.show() };
                if let Err(err) = res {
                    tracing::warn!(?err, "tray toggle pill failed");
                    return;
                }
                // Reflect the new state immediately so the next menu open
                // shows the correct verb.
                let new_label = if visible {
                    LABEL_SHOW_PILL
                } else {
                    LABEL_HIDE_PILL
                };
                let _ = pill_handle.set_text(new_label);
            }
        }
        ID_QUIT => {
            // Drain any in-flight capture cleanly (same path SIGTERM takes),
            // then mark `quitting` so `RunEvent::ExitRequested` actually
            // releases the prevent-exit guard.
            state.quitting.store(true, Ordering::SeqCst);
            let app2 = app.clone();
            let state2 = state.clone();
            tauri::async_runtime::spawn(async move {
                recording::shutdown(state2).await;
                app2.exit(0);
            });
        }
        _ => {}
    }
}

/// Subscribe to `pill:state` and keep the record-toggle label in sync.
fn install_state_listener(app: &AppHandle, record_handle: Arc<MenuItem<Wry>>) {
    app.listen("pill:state", move |event| {
        let Ok(probe) = serde_json::from_str::<PillStateProbe>(event.payload()) else {
            return;
        };
        let label = if probe.kind == "recording" {
            LABEL_STOP_RECORDING
        } else {
            LABEL_START_RECORDING
        };
        let _ = record_handle.set_text(label);
    });
}

use std::sync::Arc;

use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod db;
mod hotkey;
mod ipc;
mod pipeline;
mod recording;
mod state;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,peer=debug")))
        .with(fmt::layer().with_target(false).compact())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = Arc::new(AppState::new(&handle)?);
            app.manage(state.clone());

            position_pill(&handle)?;

            hotkey::install(handle.clone(), state.clone());

            // Background DB init.
            let db_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = db_state.db().init().await {
                    tracing::error!(?err, "db init failed");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::start_recording,
            ipc::stop_recording,
            ipc::cancel_recording,
            ipc::send_recording,
            ipc::list_recordings,
            ipc::get_recording,
            ipc::delete_recording,
            ipc::delete_all_recordings,
            ipc::open_result_window,
            ipc::set_api_key,
            ipc::get_api_key_status,
            ipc::get_hotkey_status,
            ipc::move_pill,
            ipc::cursor_position,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Peer")
        .run(|app, event| match event {
            // Closing the result window should hide it, not exit.
            // The pill is the persistent ambient surface; we only quit on
            // explicit Cmd-Q or tray "Quit".
            RunEvent::WindowEvent { label, event: WindowEvent::CloseRequested { api, .. }, .. }
                if label == "result" =>
            {
                api.prevent_close();
                if let Some(win) = app.get_webview_window("result") {
                    let _ = win.hide();
                }
            }
            // macOS: when the user clicks the dock icon and we have no
            // visible windows, restore the result window. `Reopen` fires
            // on dock activation.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { has_visible_windows, .. } => {
                if !has_visible_windows {
                    if let Some(win) = app.get_webview_window("result") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }
            RunEvent::ExitRequested { api, .. } => {
                // Don't exit when no windows are visible — we live in the pill.
                api.prevent_exit();
            }
            _ => {}
        });
}

/// First-run default position for the pill: nestled in the right edge,
/// roughly vertically centered. After this the user drags it wherever
/// they want; Tauri persists window position across launches.
fn position_pill(app: &AppHandle) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window("pill") {
        // Anchor to the bottom-right on every launch. Mid-session drags
        // still work; persisting between launches isn't worth the
        // surprise of a forgotten pill drifting offscreen on a monitor
        // change.
        anchor_default(&win)?;
    }
    Ok(())
}

fn anchor_default(win: &WebviewWindow) -> tauri::Result<()> {
    let monitor = win.current_monitor()?.or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return Ok(()) };
    let scale = monitor.scale_factor();
    let m_size = monitor.size();
    let m_pos = monitor.position();
    let logical_w = (m_size.width as f64) / scale;
    let logical_h = (m_size.height as f64) / scale;

    // Match the visible pill window dimensions in tauri.conf.json.
    let win_w = 56.0;
    let win_h = 144.0;
    // Snug to the bottom-right. Tight enough to feel anchored to the corner;
    // loose enough that an auto-hidden Dock peeking up doesn't overlap.
    let edge_margin = 12.0;
    let bottom_margin = 24.0;

    let x = (m_pos.x as f64) / scale + logical_w - win_w - edge_margin;
    let y = (m_pos.y as f64) / scale + logical_h - win_h - bottom_margin;

    win.set_position(tauri::LogicalPosition::new(x, y))?;
    Ok(())
}

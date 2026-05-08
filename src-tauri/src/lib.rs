use std::sync::Arc;

use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod binpath;
mod db;
mod hotkey;
mod ipc;
mod pipeline;
mod recording;
mod saas;
mod state;

pub use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,peer=debug")),
        )
        .with(fmt::layer().with_target(false).compact())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = Arc::new(AppState::new(&handle)?);
            app.manage(state.clone());

            #[cfg(target_os = "macos")]
            set_app_icon(&handle)?;

            #[cfg(target_os = "macos")]
            apply_result_window_vibrancy(&handle);

            position_pill(&handle)?;

            hotkey::install(handle.clone(), state.clone());

            // Background DB init.
            let db_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = db_state.db().init().await {
                    tracing::error!(?err, "db init failed");
                }
            });

            // Graceful shutdown on SIGTERM/SIGINT/SIGHUP. Without this, dev-mode
            // rebuilds (which SIGTERM the running app between recompiles) drop
            // the capture Child mid-recording — kill_on_drop SIGKILLs the
            // sidecar and the resulting mp4 has no moov atom.
            #[cfg(unix)]
            install_shutdown_hook(state.clone());

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
            ipc::open_result_window,
            ipc::set_api_key,
            ipc::get_api_key_status,
            ipc::get_account_status,
            ipc::open_account_login,
            ipc::set_device_token,
            ipc::sign_out,
            ipc::get_hotkey_status,
            ipc::set_recording_keybind,
            ipc::move_pill,
            ipc::cursor_position,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Peer")
        .run(|app, event| match event {
            // Closing the result window should hide it, not exit.
            // The pill is the persistent ambient surface; we only quit on
            // explicit Cmd-Q or tray "Quit".
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "result" => {
                api.prevent_close();
                if let Some(win) = app.get_webview_window("result") {
                    let _ = win.hide();
                }
            }
            // macOS: dock activation should always surface the main window.
            // The always-visible pill makes `has_visible_windows` unreliable
            // for this purpose, so treat every reopen as "show the result".
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                let _ = reveal_result_window(app, true);
            }
            RunEvent::ExitRequested { api, .. } => {
                // Don't exit when no windows are visible — we live in the pill.
                api.prevent_exit();
            }
            _ => {}
        });
}

pub(crate) fn reveal_result_window(app: &AppHandle, center: bool) -> tauri::Result<()> {
    let Some(win) = app.get_webview_window("result") else {
        return Ok(());
    };

    if win.is_minimized().unwrap_or(false) {
        win.unminimize()?;
    }

    win.show()?;

    if center {
        win.center()?;
    }

    #[cfg(target_os = "macos")]
    activate_app(app)?;

    win.set_focus()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_result_window_vibrancy(app: &AppHandle) {
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

    let Some(win) = app.get_webview_window("result") else {
        return;
    };

    if let Err(err) = apply_vibrancy(
        &win,
        NSVisualEffectMaterial::HudWindow,
        Some(NSVisualEffectState::Active),
        Some(14.0),
    ) {
        tracing::warn!(?err, "failed to apply result window vibrancy");
    }
}

/// Listen for the standard termination signals and flush any active capture
/// before exiting. Tauri's `RunEvent::Exit` never fires here because we
/// `prevent_exit()` unconditionally — signals are the real shutdown path.
#[cfg(unix)]
fn install_shutdown_hook(state: Arc<AppState>) {
    use tokio::signal::unix::{signal, SignalKind};
    tauri::async_runtime::spawn(async move {
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(?err, "failed to install SIGTERM handler");
                return;
            }
        };
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(?err, "failed to install SIGINT handler");
                return;
            }
        };
        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(?err, "failed to install SIGHUP handler");
                return;
            }
        };
        let signal_name = tokio::select! {
            _ = sigterm.recv() => "SIGTERM",
            _ = sigint.recv()  => "SIGINT",
            _ = sighup.recv()  => "SIGHUP",
        };
        tracing::info!(signal = signal_name, "shutdown signal received");
        recording::shutdown(state).await;
        std::process::exit(0);
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
    let monitor = win
        .current_monitor()?
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return Ok(());
    };
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

#[cfg(target_os = "macos")]
fn set_app_icon(app: &AppHandle) -> tauri::Result<()> {
    use cocoa::appkit::{NSApp, NSApplication, NSImage};
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSData;

    // When Peer is launched as a raw binary instead of a healthy .app bundle,
    // macOS does not reliably pick up the branded dock icon from bundle
    // resources. Set it explicitly so the dock matches the in-app mark.
    let icon_bytes = include_bytes!("../icons/icon.png");
    app.run_on_main_thread(move || unsafe {
        let data =
            NSData::dataWithBytes_length_(nil, icon_bytes.as_ptr().cast(), icon_bytes.len() as u64);
        let image: id = NSImage::initWithData_(NSImage::alloc(nil), data);
        let ns_app = NSApp();
        ns_app.setApplicationIconImage_(image);
    })
}

#[cfg(target_os = "macos")]
fn activate_app(app: &AppHandle) -> tauri::Result<()> {
    use cocoa::appkit::{NSApp, NSApplication};
    use cocoa::base::YES;

    app.run_on_main_thread(move || unsafe {
        let ns_app = NSApp();
        ns_app.activateIgnoringOtherApps_(YES);
    })
}

use std::sync::atomic::Ordering;
use std::{path::Path, sync::Arc};

use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod tray;

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
    load_local_env();

    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,peer=debug")),
        )
        .with(fmt::layer().with_target(false).compact())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Forward any peer:// URL handed in on relaunch — without this,
            // a click while the app is already running just reactivates the
            // dock icon and the deep link is dropped on the floor.
            for arg in args.iter().skip(1) {
                if arg.starts_with("peer://") || arg.starts_with("peer-dev://") {
                    let app2 = app.clone();
                    let url = arg.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(err) = saas::handle_deep_link(&app2, &url) {
                            tracing::warn!(?err, "deep link (single-instance) failed");
                        }
                    });
                }
            }
            let _ = reveal_result_window(app, true);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = Arc::new(AppState::new(&handle)?);
            app.manage(state.clone());

            // Register on_open_url before any slow setup work so buffered
            // launch URLs (cold-start sign-in flow) are replayed instead of
            // dropped.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let dl_handle = handle.clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        let url_string = url.to_string();
                        tracing::info!("deep link opened");
                        let app2 = dl_handle.clone();
                        if let Err(err) = saas::handle_deep_link(&app2, &url_string) {
                            tracing::warn!(?err, "deep link handler failed");
                        }
                    }
                });

                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        let url_string = url.to_string();
                        tracing::info!("deep link opened on launch");
                        if let Err(err) = saas::handle_deep_link(&handle, &url_string) {
                            tracing::warn!(?err, "deep link handler failed");
                        }
                    }
                }
            }

            #[cfg(target_os = "macos")]
            set_app_icon(&handle)?;

            #[cfg(target_os = "macos")]
            apply_result_window_vibrancy(&handle);

            position_pill(&handle)?;

            #[cfg(target_os = "macos")]
            apply_pill_all_spaces(&handle);

            tray::install(&handle, state.clone())?;

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
            ipc::retry_recording,
            ipc::list_recordings,
            ipc::get_recording,
            ipc::delete_recording,
            ipc::open_result_window,
            ipc::get_session,
            ipc::start_google_sign_in,
            ipc::sign_out,
            ipc::get_hotkey_status,
            ipc::set_recording_keybind,
            ipc::get_permission_mode,
            ipc::set_permission_mode,
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
                // The pill is the ambient surface, so closing the result window
                // shouldn't quit the app. Only let the process actually exit
                // when the user clicked the tray's Quit item, which sets the
                // `quitting` flag.
                let allow_exit = app
                    .try_state::<Arc<AppState>>()
                    .map(|s| s.quitting.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if !allow_exit {
                    api.prevent_exit();
                }
            }
            _ => {}
        });
}

fn load_local_env() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.parent().map(|p| p.join(".env.local")),
        manifest_dir.parent().map(|p| p.join(".env")),
        std::env::current_dir().ok().map(|p| p.join(".env.local")),
        std::env::current_dir().ok().map(|p| p.join(".env")),
    ];

    for path in candidates.into_iter().flatten() {
        if path.exists() {
            let _ = dotenvy::from_path(path);
        }
    }
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

/// Make the pill float in every Space and over fullscreen apps — matching
/// the behavior of Wispr Flow, Granola, Bartender, and friends.
///
/// Tauri creates a regular `NSWindow` and `alwaysOnTop: true` only sets
/// `NSFloatingWindowLevelKey` (3), which keeps the pill above siblings *within
/// the current Space*. The moment the user switches Space (or focuses a
/// fullscreen app, which lives in its own Space) the pill is left behind.
///
/// Four AppKit-level changes fix that:
///
/// 1. `styleMask |= NSWindowStyleMaskNonactivatingPanel` (1 << 7). Tells
///    AppKit the window is a non-activating panel — it won't bring our app
///    to front when clicked, and (crucially) `FullScreenAuxiliary` only
///    behaves correctly on non-activating windows. Without this bit set,
///    fullscreen Chrome / Slack / etc. paint *over* the pill instead of
///    under it. The cocoa crate doesn't surface the constant so we set it
///    via raw objc, OR'ing into the existing mask.
/// 2. `collectionBehavior = canJoinAllSpaces | fullScreenAuxiliary
///    | stationary`. canJoinAllSpaces mirrors the window into every Space;
///    fullScreenAuxiliary opts in to the synthetic fullscreen Space;
///    stationary pins it to screen coordinates instead of moving with the
///    Space (matters when users drag the pill between displays).
/// 3. `level = NSStatusWindowLevel` (25). Same tier menu-bar overlay apps
///    use — above floating, below the system menu bar.
/// 4. `hidesOnDeactivate = NO`. The pill should remain visible when the
///    user switches apps; this is the default for regular NSWindows but
///    NSPanels default to hiding, so make it explicit.
#[cfg(target_os = "macos")]
fn apply_pill_all_spaces(app: &AppHandle) {
    use cocoa::appkit::{NSWindow, NSWindowCollectionBehavior};
    use cocoa::base::id;
    use objc::{msg_send, sel, sel_impl};

    // NSWindowStyleMaskNonactivatingPanel = 1 << 7. Not exposed by cocoa 0.25,
    // so we OR the bit in via raw objc to preserve existing style flags
    // (titled, closable, resizable, …).
    const NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL: u64 = 1 << 7;

    // Look up the NSWindow handle inside the main-thread closure — the raw
    // `*mut Object` pointer is `!Send`, so capturing it from outside the
    // closure won't compile. AppHandle is cheap to clone.
    let app2 = app.clone();
    let res = app.run_on_main_thread(move || unsafe {
        let Some(win) = app2.get_webview_window("pill") else {
            return;
        };
        let ns_window: id = match win.ns_window() {
            Ok(ptr) => ptr as id,
            Err(err) => {
                tracing::warn!(?err, "pill ns_window() failed; skipping all-spaces setup");
                return;
            }
        };

        let current_mask: u64 = msg_send![ns_window, styleMask];
        let new_mask = current_mask | NS_WINDOW_STYLE_MASK_NONACTIVATING_PANEL;
        let _: () = msg_send![ns_window, setStyleMask: new_mask];

        let behavior = NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary;
        ns_window.setCollectionBehavior_(behavior);

        // NSStatusWindowLevel == 25. Above floating (3) and above the
        // synthetic fullscreen window level, but does not cover the menu bar.
        ns_window.setLevel_(25);

        // BOOL on macOS — pass the raw i8/u8 false. objc bridges this to NO.
        let _: () = msg_send![ns_window, setHidesOnDeactivate: false];

        tracing::info!(
            ?new_mask,
            "pill: non-activating panel + canJoinAllSpaces + fullScreenAuxiliary + stationary + level 25 applied"
        );
    });
    if let Err(err) = res {
        tracing::warn!(?err, "failed to apply pill all-spaces behavior");
    }
}

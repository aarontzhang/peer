use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::hotkey::HotkeyStatus;
use crate::state::AppState;

/// Max gap between Fn-down and Fn-up to count as a "tap". Longer presses
/// are treated as the user using Fn as a modifier (F-row, brightness, etc.).
const TAP_THRESHOLD: Duration = Duration::from_millis(350);

pub fn install(app: AppHandle, state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    let app_for_thread = app.clone();
    let state_for_thread = state.clone();
    std::thread::Builder::new()
        .name("peer-fn-tap".into())
        .spawn(move || run_tap(app_for_thread, state_for_thread, tx))
        .expect("spawn fn-tap thread");
}

fn publish_status(app: &AppHandle, state: &AppState, status: HotkeyStatus) {
    {
        let mut s = state.hotkey_status.lock();
        *s = status.clone();
    }
    // Loud log so the user sees it in the terminal even without RUST_LOG.
    if status.installed {
        eprintln!("[peer] Fn hotkey installed — tap Fn to record.");
    } else if let Some(reason) = &status.reason {
        eprintln!("[peer] Fn hotkey UNAVAILABLE: {reason}");
    }
    let _ = app.emit("hotkey:status", &status);
}

fn run_tap(app: AppHandle, app_state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    // RefCell because CGEventTap demands `Fn` (not `FnMut`) but the run loop
    // is single-threaded so we can safely borrow_mut from inside the callback.
    let state = RefCell::new(TapState { fn_down_at: None, other_key_pressed: false });

    let callback = move |_proxy, event_type, event: &core_graphics::event::CGEvent| {
        let mut s = state.borrow_mut();
        match event_type {
            CGEventType::FlagsChanged => {
                let flags = event.get_flags();
                let now_down = flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
                match (s.fn_down_at, now_down) {
                    (None, true) => {
                        s.fn_down_at = Some(Instant::now());
                        s.other_key_pressed = false;
                    }
                    (Some(at), false) => {
                        let elapsed = at.elapsed();
                        let other = s.other_key_pressed;
                        s.fn_down_at = None;
                        if elapsed < TAP_THRESHOLD && !other {
                            let _ = tx.send(());
                        }
                    }
                    _ => {}
                }
            }
            CGEventType::KeyDown => {
                if s.fn_down_at.is_some() {
                    s.other_key_pressed = true;
                }
            }
            _ => {}
        }
        CallbackResult::Keep
    };

    let app_loop = app.clone();
    let state_loop = app_state.clone();
    let res = CGEventTap::with_enabled(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
        callback,
        || {
            // The tap is live by the time this closure runs. Publish the
            // "installed" signal here so the UI can drop any "hotkey
            // unavailable" warning.
            publish_status(
                &app_loop,
                &state_loop,
                HotkeyStatus { installed: true, reason: None },
            );
            CFRunLoop::run_current();
        },
    );

    if res.is_err() {
        let reason = "Could not create the global event tap. Grant Peer \
                      Accessibility access in System Settings → Privacy & \
                      Security → Accessibility, then quit and reopen Peer."
            .to_string();
        tracing::warn!("Fn-tap hotkey disabled: {reason}");
        publish_status(
            &app,
            &app_state,
            HotkeyStatus { installed: false, reason: Some(reason) },
        );
    }
}

struct TapState {
    fn_down_at: Option<Instant>,
    other_key_pressed: bool,
}

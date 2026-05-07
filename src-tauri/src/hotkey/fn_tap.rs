use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::hotkey::{self, RecordingKeybind};
use crate::state::AppState;

/// Max gap between modifier-down and modifier-up to count as a tap. Longer
/// presses are treated as normal modifier use.
const TAP_THRESHOLD: Duration = Duration::from_millis(700);
const RIGHT_OPTION_KEYCODE: i64 = 61;
const RIGHT_OPTION_FLAG_BITS: u64 = 0x40;

pub fn install(app: AppHandle, state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    let app_for_thread = app.clone();
    let state_for_thread = state.clone();
    std::thread::Builder::new()
        .name("peer-modifier-tap".into())
        .spawn(move || run_tap(app_for_thread, state_for_thread, tx))
        .expect("spawn modifier-tap thread");
}

fn run_tap(app: AppHandle, app_state: Arc<AppState>, tx: mpsc::UnboundedSender<()>) {
    // RefCell because CGEventTap demands `Fn` (not `FnMut`) but the run loop
    // is single-threaded so we can safely borrow_mut from inside the callback.
    let state = RefCell::new(TapState {
        down_at: None,
        other_key_pressed: false,
    });

    let app_state_for_callback = app_state.clone();
    let callback = move |_proxy, event_type, event: &core_graphics::event::CGEvent| {
        let selected = *app_state_for_callback.recording_keybind.lock();
        let Some(target) = ModifierTapKey::from_keybind(selected) else {
            state.borrow_mut().down_at = None;
            return CallbackResult::Keep;
        };

        let mut s = state.borrow_mut();
        match event_type {
            CGEventType::FlagsChanged => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags();
                let is_active_target = s.down_at.is_some_and(|(active, _)| active == target);
                if !target.is_relevant_event(keycode, flags, is_active_target) {
                    return CallbackResult::Keep;
                }

                let now_down = target.is_down(flags);
                match (s.down_at, now_down) {
                    (None, true) => {
                        s.down_at = Some((target, Instant::now()));
                        s.other_key_pressed = false;
                    }
                    (Some((active, at)), false) if active == target => {
                        let elapsed = at.elapsed();
                        let other = s.other_key_pressed;
                        s.down_at = None;
                        if elapsed < TAP_THRESHOLD && !other {
                            let _ = tx.send(());
                        }
                    }
                    _ => {}
                }
            }
            CGEventType::KeyDown => {
                if s.down_at.is_some() {
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
            hotkey::set_modifier_tap_availability(&app_loop, &state_loop, Ok(()));
            eprintln!("[peer] Modifier-tap hotkey listener installed.");
            CFRunLoop::run_current();
        },
    );

    if res.is_err() {
        let reason = "Could not create the global modifier-key event tap. Grant Peer \
                      Accessibility access in System Settings -> Privacy & \
                      Security -> Accessibility, then quit and reopen Peer."
            .to_string();
        tracing::warn!("modifier-tap hotkey disabled: {reason}");
        eprintln!("[peer] Modifier-tap hotkey UNAVAILABLE: {reason}");
        hotkey::set_modifier_tap_availability(&app, &app_state, Err(reason));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModifierTapKey {
    RightOption,
    Fn,
}

impl ModifierTapKey {
    fn from_keybind(keybind: RecordingKeybind) -> Option<Self> {
        match keybind {
            RecordingKeybind::RightOption => Some(Self::RightOption),
            RecordingKeybind::Fn => Some(Self::Fn),
            RecordingKeybind::CmdShiftR => None,
        }
    }

    fn matches_keycode(self, keycode: i64) -> bool {
        match self {
            Self::RightOption => keycode == RIGHT_OPTION_KEYCODE,
            Self::Fn => true,
        }
    }

    fn is_relevant_event(self, keycode: i64, flags: CGEventFlags, is_active_target: bool) -> bool {
        match self {
            Self::RightOption => {
                self.matches_keycode(keycode) || right_option_flag_down(flags) || is_active_target
            }
            Self::Fn => self.matches_keycode(keycode),
        }
    }

    fn is_down(self, flags: CGEventFlags) -> bool {
        match self {
            Self::RightOption => {
                right_option_flag_down(flags) || flags.contains(CGEventFlags::CGEventFlagAlternate)
            }
            Self::Fn => flags.contains(CGEventFlags::CGEventFlagSecondaryFn),
        }
    }
}

fn right_option_flag_down(flags: CGEventFlags) -> bool {
    flags.bits() & RIGHT_OPTION_FLAG_BITS != 0
}

struct TapState {
    down_at: Option<(ModifierTapKey, Instant)>,
    other_key_pressed: bool,
}

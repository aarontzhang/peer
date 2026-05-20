//! CGEvent-backed executor for the OpenAI `computer-use-preview` action
//! schema. All event creation/posting happens synchronously on a blocking
//! task so we don't fight tokio over Send bounds — CGEvent and CGEventSource
//! are CoreFoundation handles that aren't Send-safe.

use anyhow::{anyhow, Result};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use serde_json::Value;
use std::thread::sleep;
use std::time::Duration;

/// Human-readable label for an action — surfaces in the UI status pill.
pub fn describe(action: &Value) -> String {
    let kind = action.get("type").and_then(Value::as_str).unwrap_or("?");
    match kind {
        "click" => format!(
            "Click ({:.0}, {:.0})",
            coord(action, "x"),
            coord(action, "y")
        ),
        "double_click" => format!(
            "Double-click ({:.0}, {:.0})",
            coord(action, "x"),
            coord(action, "y")
        ),
        "move" => format!(
            "Move cursor → ({:.0}, {:.0})",
            coord(action, "x"),
            coord(action, "y")
        ),
        "type" => {
            let text = action.get("text").and_then(Value::as_str).unwrap_or("");
            let preview: String = text.chars().take(40).collect();
            let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
            format!("Type \"{preview}{ellipsis}\"")
        }
        "keypress" => {
            let keys = action
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .unwrap_or_default();
            if keys.is_empty() {
                "Keypress".to_string()
            } else {
                format!("Press {keys}")
            }
        }
        "scroll" => format!(
            "Scroll dx={} dy={}",
            coord(action, "scroll_x") as i32,
            coord(action, "scroll_y") as i32
        ),
        "drag" => "Drag".to_string(),
        "wait" => "Wait".to_string(),
        "screenshot" => "Look at the screen".to_string(),
        other => format!("Action: {other}"),
    }
}

/// Execute one action. Wraps the synchronous CGEvent work in
/// `spawn_blocking` so we keep the tokio runtime healthy.
pub async fn execute(action: &Value) -> Result<()> {
    let cloned = action.clone();
    tokio::task::spawn_blocking(move || execute_sync(&cloned))
        .await
        .map_err(|err| anyhow!("action task panicked: {err}"))?
}

fn execute_sync(action: &Value) -> Result<()> {
    let kind = action
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("action missing `type` field"))?;
    match kind {
        "screenshot" => Ok(()),
        "wait" => {
            let ms = action
                .get("ms")
                .and_then(Value::as_u64)
                .unwrap_or(800)
                .min(15_000);
            sleep(Duration::from_millis(ms));
            Ok(())
        }
        "move" => move_to(coord(action, "x"), coord(action, "y")),
        "click" => click_at(
            coord(action, "x"),
            coord(action, "y"),
            mouse_button(action.get("button").and_then(Value::as_str).unwrap_or("left")),
        ),
        "double_click" => double_click_at(coord(action, "x"), coord(action, "y")),
        "type" => type_text(action.get("text").and_then(Value::as_str).unwrap_or("")),
        "keypress" => press_keys(
            action
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ),
        "scroll" => scroll(
            coord(action, "x"),
            coord(action, "y"),
            coord(action, "scroll_x") as i32,
            coord(action, "scroll_y") as i32,
        ),
        "drag" => drag(action.get("path").and_then(Value::as_array)),
        other => Err(anyhow!("unsupported computer-use action: {other}")),
    }
}

fn coord(action: &Value, field: &str) -> f64 {
    action.get(field).and_then(Value::as_f64).unwrap_or(0.0)
}

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource::new failed — is Accessibility permission granted?"))
}

fn move_to(x: f64, y: f64) -> Result<()> {
    let src = source()?;
    let event = CGEvent::new_mouse_event(
        src,
        CGEventType::MouseMoved,
        CGPoint::new(x, y),
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow!("CGEvent::new_mouse_event (move) failed"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn mouse_button(label: &str) -> CGMouseButton {
    match label.to_ascii_lowercase().as_str() {
        "right" => CGMouseButton::Right,
        "middle" => CGMouseButton::Center,
        _ => CGMouseButton::Left,
    }
}

fn click_at(x: f64, y: f64, button: CGMouseButton) -> Result<()> {
    move_to(x, y)?;
    sleep(Duration::from_millis(40));
    let (down, up) = match button {
        CGMouseButton::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp),
        CGMouseButton::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp),
        CGMouseButton::Center => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp),
    };
    let src = source()?;
    let down_evt = CGEvent::new_mouse_event(src.clone(), down, CGPoint::new(x, y), button)
        .map_err(|_| anyhow!("CGEvent::new_mouse_event (down) failed"))?;
    down_evt.post(CGEventTapLocation::HID);
    sleep(Duration::from_millis(40));
    let up_evt = CGEvent::new_mouse_event(src, up, CGPoint::new(x, y), button)
        .map_err(|_| anyhow!("CGEvent::new_mouse_event (up) failed"))?;
    up_evt.post(CGEventTapLocation::HID);
    Ok(())
}

fn double_click_at(x: f64, y: f64) -> Result<()> {
    // Two clicks within the system double-click threshold. CGEvent has a
    // ClickCount field that some apps inspect — for the POC, back-to-back
    // single clicks are reliable enough.
    click_at(x, y, CGMouseButton::Left)?;
    sleep(Duration::from_millis(60));
    click_at(x, y, CGMouseButton::Left)?;
    Ok(())
}

fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let src = source()?;
    // A blank key event with set_string injects the literal Unicode string
    // as keystrokes — the macOS-supported way to type without per-character
    // keycode mapping.
    let down = CGEvent::new_keyboard_event(src.clone(), 0, true)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event (type down) failed"))?;
    down.set_string(text);
    down.post(CGEventTapLocation::HID);
    let up = CGEvent::new_keyboard_event(src, 0, false)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event (type up) failed"))?;
    up.set_string(text);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

fn press_keys(keys: Vec<String>) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    // Split modifiers from the main key. The agent's `keys` is an array
    // representing a single chord, e.g. ["CMD", "S"].
    let mut flags = CGEventFlags::empty();
    let mut main_key: Option<CGKeyCode> = None;
    for raw in &keys {
        if let Some(flag) = modifier_flag(raw) {
            flags |= flag;
            continue;
        }
        let code = keycode_for(raw)
            .ok_or_else(|| anyhow!("no keycode mapping for key {raw:?}"))?;
        if main_key.is_some() {
            return Err(anyhow!(
                "keypress contained more than one non-modifier key: {keys:?}"
            ));
        }
        main_key = Some(code);
    }
    let Some(code) = main_key else {
        // Some chords are pure modifier taps; fire a no-op key with the flags
        // set so an app that listens for modifier-only chords sees them. For
        // POC just bail.
        return Err(anyhow!("keypress chord had no non-modifier key: {keys:?}"));
    };
    let src = source()?;
    let down = CGEvent::new_keyboard_event(src.clone(), code, true)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event (chord down) failed"))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    sleep(Duration::from_millis(40));
    let up = CGEvent::new_keyboard_event(src, code, false)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event (chord up) failed"))?;
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

fn scroll(x: f64, y: f64, dx: i32, dy: i32) -> Result<()> {
    // Move first so the scroll lands on the intended region.
    move_to(x, y)?;
    sleep(Duration::from_millis(40));
    let src = source()?;
    // OpenAI emits scroll deltas in pixels; CGScrollEventUnit::Pixel matches
    // that. Vertical = wheel1, horizontal = wheel2. macOS treats positive
    // wheel1 as scrolling "up" (content moves down), which is the inverse
    // of the agent's positive-y = scroll-down convention — flip the sign so
    // typical "scroll the page down" maps to the right gesture.
    let event = CGEvent::new_scroll_event(src, ScrollEventUnit::PIXEL, 2, -dy, dx, 0)
        .map_err(|_| anyhow!("CGEvent::new_scroll_event failed"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn drag(path: Option<&Vec<Value>>) -> Result<()> {
    let Some(pts) = path else {
        return Err(anyhow!("drag action missing `path`"));
    };
    if pts.len() < 2 {
        return Err(anyhow!("drag path needs at least 2 points"));
    }
    let extract = |v: &Value| -> Result<CGPoint> {
        let x = v
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("drag point missing x"))?;
        let y = v
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("drag point missing y"))?;
        Ok(CGPoint::new(x, y))
    };
    let src = source()?;
    let start = extract(&pts[0])?;
    move_to(start.x, start.y)?;
    sleep(Duration::from_millis(40));
    let down = CGEvent::new_mouse_event(
        src.clone(),
        CGEventType::LeftMouseDown,
        start,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow!("CGEvent::new_mouse_event (drag down) failed"))?;
    down.post(CGEventTapLocation::HID);
    for v in pts.iter().skip(1) {
        let pt = extract(v)?;
        let drag_evt = CGEvent::new_mouse_event(
            src.clone(),
            CGEventType::LeftMouseDragged,
            pt,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow!("CGEvent::new_mouse_event (drag step) failed"))?;
        drag_evt.post(CGEventTapLocation::HID);
        sleep(Duration::from_millis(20));
    }
    let end = extract(pts.last().unwrap())?;
    let up = CGEvent::new_mouse_event(src, CGEventType::LeftMouseUp, end, CGMouseButton::Left)
        .map_err(|_| anyhow!("CGEvent::new_mouse_event (drag up) failed"))?;
    up.post(CGEventTapLocation::HID);
    Ok(())
}

fn modifier_flag(name: &str) -> Option<CGEventFlags> {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match normalized.as_str() {
        "CMD" | "COMMAND" | "META" | "SUPER" | "WIN" => Some(CGEventFlags::CGEventFlagCommand),
        "CTRL" | "CONTROL" => Some(CGEventFlags::CGEventFlagControl),
        "ALT" | "OPTION" | "OPT" => Some(CGEventFlags::CGEventFlagAlternate),
        "SHIFT" => Some(CGEventFlags::CGEventFlagShift),
        _ => None,
    }
}

fn keycode_for(name: &str) -> Option<CGKeyCode> {
    use core_graphics::event::KeyCode;
    // Strip non-alphanumeric chars and uppercase so e.g. "arrow-down",
    // "Arrow Down", "ARROW_DOWN", "arrowdown" all collapse to "ARROWDOWN".
    let normalized: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    Some(match normalized.as_str() {
        "RETURN" | "ENTER" => KeyCode::RETURN,
        "TAB" => KeyCode::TAB,
        "SPACE" | "SPACEBAR" => KeyCode::SPACE,
        "BACKSPACE" | "DELETE" => KeyCode::DELETE,
        "FORWARDDELETE" | "DEL" => 0x75,
        "ESC" | "ESCAPE" => KeyCode::ESCAPE,
        "ARROWLEFT" | "LEFT" => 0x7B,
        "ARROWRIGHT" | "RIGHT" => 0x7C,
        "ARROWDOWN" | "DOWN" => 0x7D,
        "ARROWUP" | "UP" => 0x7E,
        "HOME" => 0x73,
        "END" => 0x77,
        "PAGEUP" => 0x74,
        "PAGEDOWN" => 0x79,
        "F1" => 0x7A,
        "F2" => 0x78,
        "F3" => 0x63,
        "F4" => 0x76,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F8" => 0x64,
        "F9" => 0x65,
        "F10" => 0x6D,
        "F11" => 0x67,
        "F12" => 0x6F,
        "A" => KeyCode::ANSI_A,
        "B" => KeyCode::ANSI_B,
        "C" => KeyCode::ANSI_C,
        "D" => KeyCode::ANSI_D,
        "E" => KeyCode::ANSI_E,
        "F" => KeyCode::ANSI_F,
        "G" => KeyCode::ANSI_G,
        "H" => KeyCode::ANSI_H,
        "I" => KeyCode::ANSI_I,
        "J" => KeyCode::ANSI_J,
        "K" => KeyCode::ANSI_K,
        "L" => KeyCode::ANSI_L,
        "M" => KeyCode::ANSI_M,
        "N" => KeyCode::ANSI_N,
        "O" => KeyCode::ANSI_O,
        "P" => KeyCode::ANSI_P,
        "Q" => KeyCode::ANSI_Q,
        "R" => KeyCode::ANSI_R,
        "S" => KeyCode::ANSI_S,
        "T" => KeyCode::ANSI_T,
        "U" => KeyCode::ANSI_U,
        "V" => KeyCode::ANSI_V,
        "W" => KeyCode::ANSI_W,
        "X" => KeyCode::ANSI_X,
        "Y" => KeyCode::ANSI_Y,
        "Z" => KeyCode::ANSI_Z,
        "0" => KeyCode::ANSI_0,
        "1" => KeyCode::ANSI_1,
        "2" => KeyCode::ANSI_2,
        "3" => KeyCode::ANSI_3,
        "4" => KeyCode::ANSI_4,
        "5" => KeyCode::ANSI_5,
        "6" => KeyCode::ANSI_6,
        "7" => KeyCode::ANSI_7,
        "8" => KeyCode::ANSI_8,
        "9" => KeyCode::ANSI_9,
        "MINUS" | "HYPHEN" => KeyCode::ANSI_MINUS,
        "EQUAL" | "EQUALS" => KeyCode::ANSI_EQUAL,
        "LEFTBRACKET" | "LEFTSQUAREBRACKET" => KeyCode::ANSI_LEFT_BRACKET,
        "RIGHTBRACKET" | "RIGHTSQUAREBRACKET" => KeyCode::ANSI_RIGHT_BRACKET,
        "BACKSLASH" => KeyCode::ANSI_BACKSLASH,
        "SEMICOLON" => KeyCode::ANSI_SEMICOLON,
        "QUOTE" | "APOSTROPHE" => KeyCode::ANSI_QUOTE,
        "COMMA" => KeyCode::ANSI_COMMA,
        "PERIOD" | "DOT" => KeyCode::ANSI_PERIOD,
        "SLASH" => KeyCode::ANSI_SLASH,
        "GRAVE" | "BACKTICK" => KeyCode::ANSI_GRAVE,
        _ => return single_punctuation_keycode(name),
    })
}

/// Fallback for one-character key names like "-", "/", "[", "`" that the
/// alphanumeric normalizer would have stripped. Lets a key payload of `["/"]`
/// still produce a real keystroke without needing a named alias.
fn single_punctuation_keycode(name: &str) -> Option<CGKeyCode> {
    use core_graphics::event::KeyCode;
    if name.chars().count() != 1 {
        return None;
    }
    Some(match name {
        "-" => KeyCode::ANSI_MINUS,
        "=" => KeyCode::ANSI_EQUAL,
        "[" => KeyCode::ANSI_LEFT_BRACKET,
        "]" => KeyCode::ANSI_RIGHT_BRACKET,
        "\\" => KeyCode::ANSI_BACKSLASH,
        ";" => KeyCode::ANSI_SEMICOLON,
        "'" => KeyCode::ANSI_QUOTE,
        "," => KeyCode::ANSI_COMMA,
        "." => KeyCode::ANSI_PERIOD,
        "/" => KeyCode::ANSI_SLASH,
        "`" => KeyCode::ANSI_GRAVE,
        _ => return None,
    })
}

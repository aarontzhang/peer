//! Toggle macOS Accessibility cursor scale during recording so that the
//! pointer is unmistakably visible in keyframes. We bump it to 4x and
//! restore the prior value on stop. Uses the `defaults` CLI rather than
//! talking to UserDefaults directly because the universalaccess domain
//! is not in our sandbox bubble.
//!
//! CAVEAT: WindowServer reads `mouseDriverCursorSize` lazily. The value
//! does take effect mid-session on recent macOS versions, but only after
//! some accessibility-subsystem activity (moving the pointer through a
//! menu, clicking, etc.) — pure `defaults write` alone occasionally
//! lands without an immediate visual change. If the pointer in your
//! keyframes still looks default-sized, open System Settings →
//! Accessibility → Display → Pointer once to "prime" the WindowServer;
//! after that, the size flip during recording is reliable.

use std::process::Command;
use std::sync::OnceLock;

const DOMAIN: &str = "com.apple.universalaccess";
const KEY: &str = "mouseDriverCursorSize";
const TARGET: &str = "4.0";

static PRIOR: OnceLock<parking_lot::Mutex<Option<String>>> = OnceLock::new();

fn prior() -> &'static parking_lot::Mutex<Option<String>> {
    PRIOR.get_or_init(|| parking_lot::Mutex::new(None))
}

pub fn enlarge() {
    let current = read_current();
    *prior().lock() = current;
    let _ = Command::new("defaults")
        .args(["write", DOMAIN, KEY, "-float", TARGET])
        .status();
}

pub fn restore() {
    let mut guard = prior().lock();
    match guard.take() {
        Some(value) => {
            let _ = Command::new("defaults")
                .args(["write", DOMAIN, KEY, "-float", &value])
                .status();
        }
        None => {
            let _ = Command::new("defaults")
                .args(["delete", DOMAIN, KEY])
                .status();
        }
    }
}

fn read_current() -> Option<String> {
    let out = Command::new("defaults")
        .args(["read", DOMAIN, KEY])
        .output()
        .ok()?;
    if !out.status.success() { return None }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

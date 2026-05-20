//! Screen capture for the automation loop.
//!
//! macOS only. Uses `screencapture` + `sips` (both shipped with the OS) so we
//! don't pull in an image-processing crate just to downscale. The agent
//! requires the image dimensions to match `display_width` / `display_height`,
//! which on a Retina display means resizing the raw 2x capture to logical
//! points.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use tokio::process::Command;

use core_graphics::display::CGDisplay;

/// Logical main-display size in points. Matches the coordinate space used by
/// `CGEvent` for mouse positioning, so the model can return coords we can post
/// directly.
pub fn main_display_logical_size() -> Result<(u32, u32)> {
    let display = CGDisplay::main();
    let mode = display
        .display_mode()
        .ok_or_else(|| anyhow!("CGDisplay::display_mode returned None for the main display"))?;
    let width = mode.width() as u32;
    let height = mode.height() as u32;
    if width == 0 || height == 0 {
        return Err(anyhow!(
            "main display reported zero-size mode ({width}x{height})"
        ));
    }
    Ok((width, height))
}

/// Capture the main display, resize to the supplied logical dimensions, and
/// return a base64 PNG ready to ship in JSON.
pub async fn screenshot_base64_at(width: u32, height: u32) -> Result<String> {
    let dir = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let raw_path = dir.join(format!("peer-automation-{stamp}.png"));
    let scaled_path = dir.join(format!("peer-automation-{stamp}-s.png"));

    let raw_status = Command::new("/usr/sbin/screencapture")
        .arg("-x") // silent — no shutter sound
        .arg("-C") // capture the cursor
        .arg("-m") // main display only
        .arg("-t")
        .arg("png")
        .arg(&raw_path)
        .status()
        .await
        .context("spawning screencapture")?;
    if !raw_status.success() {
        cleanup(&raw_path).await;
        return Err(anyhow!(
            "screencapture failed (status {})",
            raw_status.code().unwrap_or(-1)
        ));
    }

    // `sips --resampleHeightWidth h w` forces the exact dimensions the agent
    // expects. Retina captures are typically 2x; this brings them back to
    // logical points so the coordinate spaces line up.
    let resize_status = Command::new("/usr/bin/sips")
        .arg("--resampleHeightWidth")
        .arg(height.to_string())
        .arg(width.to_string())
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(&raw_path)
        .arg("--out")
        .arg(&scaled_path)
        .status()
        .await
        .context("spawning sips for screenshot resize")?;
    if !resize_status.success() {
        cleanup(&raw_path).await;
        cleanup(&scaled_path).await;
        return Err(anyhow!(
            "sips failed to resize screenshot (status {})",
            resize_status.code().unwrap_or(-1)
        ));
    }

    let bytes = tokio::fs::read(&scaled_path)
        .await
        .with_context(|| format!("reading resized screenshot at {}", scaled_path.display()))?;
    cleanup(&raw_path).await;
    cleanup(&scaled_path).await;

    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

async fn cleanup(path: &std::path::Path) {
    let _ = tokio::fs::remove_file(path).await;
}

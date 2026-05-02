use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Default)]
pub struct Probe {
    pub duration_secs: f64,
}

#[derive(Deserialize)]
struct FormatJson { format: Format }
#[derive(Deserialize)]
struct Format { duration: Option<String> }

pub async fn probe(path: &Path) -> Result<Probe> {
    let out = Command::new("ffprobe")
        .args(["-hide_banner", "-v", "error", "-show_format", "-print_format", "json"])
        .arg(path)
        .output()
        .await
        .context("invoking ffprobe")?;
    if !out.status.success() {
        anyhow::bail!("ffprobe failed: {}", super::tail_stderr(&out.stderr));
    }
    let parsed: FormatJson = serde_json::from_slice(&out.stdout).unwrap_or(FormatJson { format: Format { duration: None } });
    let duration_secs = parsed.format.duration
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    Ok(Probe { duration_secs })
}

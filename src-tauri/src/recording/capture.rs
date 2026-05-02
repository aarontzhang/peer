//! Drives the bundled Swift `PeerCapture` sidecar over stdin/stdout.
//! Falls back to `ffmpeg avfoundation` capture if the sidecar is missing —
//! useful in `pnpm tauri dev` before the sidecar has been built.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

pub struct CaptureProcess {
    child: Child,
    backend: Backend,
}

#[derive(Copy, Clone, Debug)]
enum Backend {
    Sidecar,
    Ffmpeg,
}

pub async fn start(bin_dir: &Path, output: &Path) -> Result<CaptureProcess> {
    if let Some(path) = locate_sidecar(bin_dir) {
        return start_sidecar(&path, output).await;
    }
    tracing::warn!("PeerCapture sidecar not found; falling back to ffmpeg avfoundation");
    start_ffmpeg(output).await
}

fn locate_sidecar(bin_dir: &Path) -> Option<PathBuf> {
    for name in [
        "PeerCapture-aarch64-apple-darwin",
        "PeerCapture-x86_64-apple-darwin",
        "PeerCapture",
    ] {
        let p = bin_dir.join(name);
        if p.exists() { return Some(p); }
    }
    // Dev-mode lookup
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bin")
        .join("PeerCapture-aarch64-apple-darwin");
    if dev.exists() { return Some(dev); }
    None
}

async fn start_sidecar(path: &Path, output: &Path) -> Result<CaptureProcess> {
    let mut child = Command::new(path)
        .arg("--output").arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Wait for "READY" on stdout so we don't stop before capture has started.
    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target: "capture", "{line}");
                if line.trim() == "READY" { return Ok::<_, anyhow::Error>(()) }
            }
            Err(anyhow!("sidecar exited before READY"))
        }).await;
        timeout.map_err(|_| anyhow!("sidecar startup timed out"))??;
    }

    Ok(CaptureProcess { child, backend: Backend::Sidecar })
}

async fn start_ffmpeg(output: &Path) -> Result<CaptureProcess> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"])
        .arg("-y")
        .args(["-f", "avfoundation"])
        .args(["-framerate", "30"])
        .args(["-pixel_format", "uyvy422"])
        .args(["-i", "1:0"]) // primary screen + default mic
        .args(["-c:v", "h264_videotoolbox"])
        .args(["-b:v", "5M"])
        .args(["-c:a", "aac"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let child = cmd.spawn()?;
    Ok(CaptureProcess { child, backend: Backend::Ffmpeg })
}

impl CaptureProcess {
    pub async fn stop(&mut self) -> Result<()> {
        match self.backend {
            Backend::Sidecar => {
                if let Some(stdin) = self.child.stdin.as_mut() {
                    let _ = stdin.write_all(b"STOP\n").await;
                    let _ = stdin.flush().await;
                }
            }
            Backend::Ffmpeg => {
                // Send 'q' to ffmpeg's stdin for graceful stop.
                if let Some(stdin) = self.child.stdin.as_mut() {
                    let _ = stdin.write_all(b"q").await;
                    let _ = stdin.flush().await;
                }
            }
        }
        // Give the encoder a moment to flush; then ensure exit.
        let exit = tokio::time::timeout(std::time::Duration::from_secs(8), self.child.wait()).await;
        match exit {
            Ok(Ok(status)) => {
                if !status.success() {
                    tracing::warn!("capture exited with {status}");
                }
                Ok(())
            }
            _ => {
                let _ = self.child.kill().await;
                Ok(())
            }
        }
    }

    pub async fn cancel(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

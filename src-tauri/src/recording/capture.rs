//! Drives the bundled Swift `PeerCapture` sidecar over stdin/stdout.
//! Falls back to `ffmpeg avfoundation` capture if the sidecar is missing —
//! useful in `pnpm tauri dev` before the sidecar has been built.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

pub struct CaptureProcess {
    child: Child,
    backend: Backend,
    stderr_tail: LogTail,
}

#[derive(Copy, Clone, Debug)]
enum Backend {
    Sidecar,
    Ffmpeg,
}

#[derive(Clone, Default)]
struct LogTail {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogTail {
    fn push(&self, line: String) {
        let Ok(mut lines) = self.lines.lock() else {
            return;
        };
        if lines.len() == 12 {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    fn summary(&self) -> String {
        let Ok(lines) = self.lines.lock() else {
            return String::new();
        };
        lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

pub async fn start(bin_dir: &Path, output: &Path) -> Result<CaptureProcess> {
    if let Some(path) = locate_sidecar(bin_dir) {
        return start_sidecar(&path, output).await;
    }
    tracing::warn!("PeerCapture sidecar not found; falling back to ffmpeg avfoundation");
    start_ffmpeg(output).await
}

fn locate_sidecar(bin_dir: &Path) -> Option<PathBuf> {
    let names = [
        "PeerCapture-aarch64-apple-darwin",
        "PeerCapture-x86_64-apple-darwin",
        "PeerCapture",
    ];
    for name in names {
        let p = bin_dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    // Bundled-app lookup: Tauri's externalBin places sidecars next to the
    // main binary in Contents/MacOS/, not in Resources/bin/.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let p = dir.join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    // Dev-mode lookup
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bin")
        .join("PeerCapture-aarch64-apple-darwin");
    if dev.exists() {
        return Some(dev);
    }
    None
}

async fn start_sidecar(path: &Path, output: &Path) -> Result<CaptureProcess> {
    let mut child = Command::new(path)
        .arg("--output")
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let stderr_tail = LogTail::default();
    if let Some(stderr) = child.stderr.take() {
        let stderr_tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::warn!(target: "capture", "sidecar stderr: {line}");
                stderr_tail.push(line);
            }
        });
    }

    if let Some(stdout) = child.stdout.take() {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            let mut ready_tx = Some(ready_tx);
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target: "capture", "{line}");
                if line.trim() == "READY" {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }
        });
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx).await;
        match timeout {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(anyhow!("sidecar exited before READY")),
            Err(_) => return Err(anyhow!("sidecar startup timed out")),
        }
    }

    Ok(CaptureProcess {
        child,
        backend: Backend::Sidecar,
        stderr_tail,
    })
}

async fn start_ffmpeg(output: &Path) -> Result<CaptureProcess> {
    let (audio_index, audio_name) = preferred_ffmpeg_audio_device().await;
    tracing::info!(
        target: "capture",
        "ffmpeg fallback using avfoundation audio device {audio_index}: {audio_name}"
    );

    let mut cmd = Command::new(crate::binpath::ffmpeg());
    cmd.args(["-hide_banner", "-loglevel", "error"])
        .arg("-y")
        .args(["-f", "avfoundation"])
        .args(["-framerate", "30"])
        .args(["-pixel_format", "uyvy422"])
        .arg("-i")
        .arg(format!("1:{audio_index}")) // primary screen + preferred mic
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
    Ok(CaptureProcess {
        child,
        backend: Backend::Ffmpeg,
        stderr_tail: LogTail::default(),
    })
}

async fn preferred_ffmpeg_audio_device() -> (String, String) {
    let devices = ffmpeg_audio_devices().await;

    if let Some(device) = devices
        .iter()
        .find(|d| is_builtin_mac_microphone_name(&d.name))
    {
        return (device.index.clone(), device.name.clone());
    }

    if let Some(device) = devices
        .iter()
        .find(|d| is_plausible_physical_microphone_name(&d.name))
    {
        return (device.index.clone(), device.name.clone());
    }

    devices
        .into_iter()
        .next()
        .map(|d| (d.index, d.name))
        .unwrap_or_else(|| ("0".to_string(), "default".to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FfmpegAudioDevice {
    index: String,
    name: String,
}

async fn ffmpeg_audio_devices() -> Vec<FfmpegAudioDevice> {
    let output = match Command::new(crate::binpath::ffmpeg())
        .args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ])
        .output()
        .await
    {
        Ok(output) => output,
        Err(err) => {
            tracing::warn!(target: "capture", ?err, "failed to enumerate avfoundation audio devices");
            return Vec::new();
        }
    };

    parse_ffmpeg_audio_devices(&String::from_utf8_lossy(&output.stderr))
}

fn parse_ffmpeg_audio_devices(stderr: &str) -> Vec<FfmpegAudioDevice> {
    let mut in_audio_section = false;
    let mut devices = Vec::new();

    for line in stderr.lines() {
        if line.contains("AVFoundation audio devices:") {
            in_audio_section = true;
            continue;
        }
        if line.contains("AVFoundation video devices:") {
            in_audio_section = false;
            continue;
        }
        if !in_audio_section {
            continue;
        }

        let Some(last_open) = line.rfind('[') else {
            continue;
        };
        let Some(close_rel) = line[last_open..].find(']') else {
            continue;
        };
        let close = last_open + close_rel;
        let index = line[last_open + 1..close].trim();
        if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let name = line.get(close + 1..).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }

        devices.push(FfmpegAudioDevice {
            index: index.to_string(),
            name: name.to_string(),
        });
    }

    devices
}

fn is_builtin_mac_microphone_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let mentions_mic = lower.contains("microphone") || lower.contains("mic");
    mentions_mic
        && (lower.contains("built-in")
            || lower.contains("built in")
            || lower.contains("macbook")
            || lower.contains("internal"))
}

fn is_plausible_physical_microphone_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let mentions_mic = lower.contains("microphone") || lower.contains("mic");
    let obviously_virtual_or_bluetooth = [
        "airpods",
        "beats",
        "bluetooth",
        "teams",
        "zoom",
        "virtual",
        "aggregate",
        "display",
        "speaker",
        "output",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    mentions_mic && !obviously_virtual_or_bluetooth
}

impl CaptureProcess {
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().map_err(Into::into)
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(status) = self.child.try_wait()? {
            let details = self.stderr_tail.summary();
            if details.is_empty() {
                return Err(anyhow!("capture process exited before stop: {status}"));
            }
            return Err(anyhow!(
                "capture process exited before stop: {status}: {details}"
            ));
        }

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
        // Give the encoder time to flush the MP4 trailer. Killing here leaves
        // an mdat-only file with no moov atom, so timeout is an explicit error.
        let timeout = match self.backend {
            Backend::Sidecar => std::time::Duration::from_secs(60),
            Backend::Ffmpeg => std::time::Duration::from_secs(30),
        };
        let exit = tokio::time::timeout(timeout, self.child.wait()).await;
        match exit {
            Ok(Ok(status)) => {
                if !status.success() {
                    let details = self.stderr_tail.summary();
                    if details.is_empty() {
                        return Err(anyhow!("capture exited with {status}"));
                    }
                    return Err(anyhow!("capture exited with {status}: {details}"));
                }
                Ok(())
            }
            Ok(Err(err)) => Err(err.into()),
            Err(_) => {
                let _ = self.child.kill().await;
                Err(anyhow!("capture finalization timed out after {timeout:?}"))
            }
        }
    }

    pub async fn cancel(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_builtin_mac_microphone_name, is_plausible_physical_microphone_name,
        parse_ffmpeg_audio_devices, FfmpegAudioDevice,
    };

    #[test]
    fn parses_ffmpeg_audio_device_listing() {
        let stderr = r#"
[AVFoundation input device @ 0x123] AVFoundation video devices:
[AVFoundation input device @ 0x123] [0] FaceTime HD Camera
[AVFoundation input device @ 0x123] [1] Capture screen 0
[AVFoundation input device @ 0x123] AVFoundation audio devices:
[AVFoundation input device @ 0x123] [0] Aaron's Airpods
[AVFoundation input device @ 0x123] [1] MacBook Pro Microphone
[AVFoundation input device @ 0x123] [2] Microsoft Teams Audio
"#;

        let devices = parse_ffmpeg_audio_devices(stderr);

        assert_eq!(
            devices,
            vec![
                FfmpegAudioDevice {
                    index: "0".into(),
                    name: "Aaron's Airpods".into()
                },
                FfmpegAudioDevice {
                    index: "1".into(),
                    name: "MacBook Pro Microphone".into()
                },
                FfmpegAudioDevice {
                    index: "2".into(),
                    name: "Microsoft Teams Audio".into()
                },
            ]
        );
    }

    #[test]
    fn recognizes_builtin_mac_microphone_names() {
        assert!(is_builtin_mac_microphone_name("MacBook Pro Microphone"));
        assert!(is_builtin_mac_microphone_name("Built-in Microphone"));
        assert!(!is_builtin_mac_microphone_name("Aaron's Airpods"));
    }

    #[test]
    fn filters_virtual_or_bluetooth_microphones() {
        assert!(is_plausible_physical_microphone_name("USB Microphone"));
        assert!(!is_plausible_physical_microphone_name("Aaron's Airpods"));
        assert!(!is_plausible_physical_microphone_name(
            "Microsoft Teams Audio"
        ));
    }
}

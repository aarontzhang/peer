//! Smart keyframe selection — scene-detect first, uniform-sample fallback.
//! Mirrors `~/Desktop/Projects/video-analyzer/lib/frameExtractor.ts` but adds
//! the scene-detect step the plan calls for.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;

const SCENE_THRESHOLD: f32 = 0.25;
const MIN_FRAMES_FOR_SCENE: usize = 8;
const MAX_FRAMES: usize = 40;
const FALLBACK_FPS: u32 = 2;
const MAX_WIDTH: u32 = 1280;
const QUALITY: u32 = 3;

#[derive(Debug, Clone)]
pub struct Keyframe {
    pub path: PathBuf,
    /// Best-effort timestamp inferred from frame index. Real timestamps would
    /// require parsing showinfo stderr; the analyzer model only needs ordering.
    pub approx_t: f32,
}

pub async fn extract(video: &Path, out_dir: &Path) -> Result<Vec<Keyframe>> {
    tokio::fs::create_dir_all(out_dir).await.ok();
    clear_dir(out_dir).await?;

    // Pass 1 — scene detect. Lenient: when no frames cross the threshold
    // (e.g., a short or static recording) the image2 muxer exits non-zero
    // with "Nothing was written into output file". Treat that — and any
    // other pass-1 failure — as "0 scene frames" and fall through to the
    // uniform-sampling fallback rather than aborting the pipeline.
    let pattern = out_dir.join("scene_%04d.jpg");
    let filter = format!(
        "select='gt(scene,{thresh})',scale={w}:-2:force_original_aspect_ratio=decrease",
        thresh = SCENE_THRESHOLD,
        w = MAX_WIDTH,
    );
    let scene_ok = try_ffmpeg(&[
        "-hide_banner",
        "-loglevel", "error",
        "-y",
        "-i", video.to_string_lossy().as_ref(),
        "-vf", &filter,
        "-fps_mode", "vfr",
        "-q:v", &QUALITY.to_string(),
        pattern.to_string_lossy().as_ref(),
    ]).await;
    if let Err(err) = &scene_ok {
        tracing::info!(?err, "scene detect produced no frames — falling back");
    }

    let mut frames = list_jpegs(out_dir, "scene_").await.unwrap_or_default();

    if frames.len() < MIN_FRAMES_FOR_SCENE {
        tracing::info!(found = frames.len(), "scene detect underflow — falling back to uniform fps={FALLBACK_FPS}");
        clear_dir(out_dir).await?;
        let pattern = out_dir.join("uni_%04d.jpg");
        let filter = format!(
            "fps={fps},scale={w}:-2:force_original_aspect_ratio=decrease",
            fps = FALLBACK_FPS,
            w = MAX_WIDTH,
        );
        let _ = try_ffmpeg(&[
            "-hide_banner",
            "-loglevel", "error",
            "-y",
            "-i", video.to_string_lossy().as_ref(),
            "-vf", &filter,
            "-q:v", &QUALITY.to_string(),
            pattern.to_string_lossy().as_ref(),
        ]).await;
        frames = list_jpegs(out_dir, "uni_").await.unwrap_or_default();

        // Last resort for sub-second clips: grab a single frame at t=0.
        // Better an "incomplete" analysis than a hard pipeline failure.
        if frames.is_empty() {
            let pattern = out_dir.join("uni_0001.jpg");
            let _ = try_ffmpeg(&[
                "-hide_banner",
                "-loglevel", "error",
                "-y",
                "-i", video.to_string_lossy().as_ref(),
                "-frames:v", "1",
                "-q:v", &QUALITY.to_string(),
                pattern.to_string_lossy().as_ref(),
            ]).await;
            frames = list_jpegs(out_dir, "uni_").await.unwrap_or_default();
        }
    }

    // Cap at MAX_FRAMES. If we have more, uniformly downsample.
    if frames.len() > MAX_FRAMES {
        let step = frames.len() as f32 / MAX_FRAMES as f32;
        let mut kept = Vec::with_capacity(MAX_FRAMES);
        for i in 0..MAX_FRAMES {
            let idx = (i as f32 * step).floor() as usize;
            kept.push(frames[idx].clone());
        }
        // Delete dropped files for tidiness.
        let kept_set: std::collections::HashSet<_> = kept.iter().cloned().collect();
        for p in &frames {
            if !kept_set.contains(p) {
                let _ = tokio::fs::remove_file(p).await;
            }
        }
        frames = kept;
    }

    let count = frames.len() as f32;
    let frames = frames
        .into_iter()
        .enumerate()
        .map(|(i, path)| Keyframe {
            path,
            approx_t: if count > 1.0 { i as f32 / (count - 1.0) } else { 0.0 },
        })
        .collect();
    Ok(frames)
}

async fn run_ffmpeg(args: &[&str]) -> Result<()> {
    let out = Command::new("ffmpeg")
        .args(args)
        .output()
        .await
        .context("spawning ffmpeg")?;
    if !out.status.success() {
        anyhow::bail!("ffmpeg failed: {}", super::tail_stderr(&out.stderr));
    }
    Ok(())
}

/// Same as `run_ffmpeg` but returns the error rather than bailing — used for
/// passes where a non-zero exit is recoverable (e.g., scene-detect producing
/// zero frames).
async fn try_ffmpeg(args: &[&str]) -> Result<()> {
    run_ffmpeg(args).await
}

async fn list_jpegs(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jpg") {
            if let Some(stem) = p.file_name().and_then(|s| s.to_str()) {
                if stem.starts_with(prefix) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

async fn clear_dir(dir: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("jpg") {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
    Ok(())
}

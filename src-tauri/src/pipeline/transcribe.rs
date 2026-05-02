//! Whisper-based parallel chunked transcription.
//!
//! Pattern adapted from `~/Desktop/Projects/Autocut/lib/transcriptionUtils.ts`
//! (`buildOverlappingRanges` + `dedupeCaptionEntries`). The Autocut version
//! was sequential — we run chunks concurrently with a semaphore for sub-5s
//! end-to-end on a 60s clip.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::Semaphore;

const CHUNK_SECONDS: f64 = 30.0;
const OVERLAP_SECONDS: f64 = 0.75;
const DEDUPE_TOLERANCE_SECONDS: f64 = 0.08;
const MAX_PARALLEL: usize = 8;

#[derive(Debug, Clone)]
pub struct CaptionEntry {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub entries: Vec<CaptionEntry>,
}

impl Transcript {
    pub fn plain_text(&self) -> String {
        self.entries
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn slice(&self, start: f64, end: f64) -> String {
        self.entries
            .iter()
            .filter(|e| e.end >= start && e.start <= end)
            .map(|e| format!("[{:.1}s] {}", e.start, e.text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: Option<String>,
    segments: Option<Vec<WhisperSegment>>,
}

#[derive(Deserialize)]
struct WhisperSegment {
    start: f64,
    end: f64,
    text: String,
}

pub async fn transcribe(video: &Path, openai_key: &str, total_secs: f64) -> Result<Transcript> {
    let audio_path = extract_audio(video).await?;
    if total_secs <= 0.5 {
        return Ok(Transcript::default());
    }
    let ranges = build_overlapping_ranges(0.0, total_secs, CHUNK_SECONDS, OVERLAP_SECONDS);
    if ranges.is_empty() {
        return Ok(Transcript::default());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let sem = Arc::new(Semaphore::new(MAX_PARALLEL));

    let mut tasks = FuturesUnordered::new();
    for (i, range) in ranges.iter().enumerate() {
        let sem = sem.clone();
        let client = client.clone();
        let key = openai_key.to_string();
        let src = audio_path.clone();
        let range = *range;
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            transcribe_range(&client, &key, &src, range).await.map(|e| (i, e))
        }));
    }

    let mut all = Vec::new();
    while let Some(joined) = tasks.next().await {
        match joined {
            Ok(Ok((_, entries))) => all.extend(entries),
            Ok(Err(err)) => tracing::warn!(?err, "whisper chunk failed (continuing)"),
            Err(err) => tracing::warn!(?err, "whisper task panicked"),
        }
    }

    Ok(Transcript { entries: dedupe_captions(all, DEDUPE_TOLERANCE_SECONDS) })
}

async fn extract_audio(video: &Path) -> Result<PathBuf> {
    let out = video.with_extension("mp3");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-b:a", "64k"])
        .arg(&out)
        .output()
        .await
        .context("running ffmpeg for audio extraction")?;
    if !status.status.success() {
        anyhow::bail!("audio extraction failed: {}", super::tail_stderr(&status.stderr));
    }
    Ok(out)
}

async fn transcribe_range(
    client: &reqwest::Client,
    key: &str,
    audio: &Path,
    range: TimeRange,
) -> Result<Vec<CaptionEntry>> {
    let chunk_path = std::env::temp_dir().join(format!(
        "hb-{}-{:.0}-{:.0}.mp3",
        std::process::id(),
        range.start * 1000.0,
        range.end * 1000.0,
    ));
    let dur = (range.end - range.start).max(0.1);
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y",
               "-ss", &format!("{:.3}", range.start), "-t", &format!("{:.3}", dur), "-i"])
        .arg(audio)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-b:a", "64k"])
        .arg(&chunk_path)
        .output()
        .await?;
    if !status.status.success() {
        anyhow::bail!("ffmpeg slice failed: {}", super::tail_stderr(&status.stderr));
    }

    let bytes = tokio::fs::read(&chunk_path).await?;
    let _ = tokio::fs::remove_file(&chunk_path).await;
    let part = Part::bytes(bytes)
        .file_name("audio.mp3")
        .mime_str("audio/mpeg")?;
    let form = Form::new()
        .part("file", part)
        .text("model", "whisper-1")
        .text("response_format", "verbose_json")
        .text("temperature", "0");

    let res = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!("whisper {} — {}", status, body));
    }

    let parsed: WhisperResponse = res.json().await?;
    let entries = if let Some(segs) = parsed.segments {
        segs.into_iter()
            .map(|s| CaptionEntry {
                start: range.start + s.start,
                end: range.start + s.end,
                text: s.text.trim().to_string(),
            })
            .filter(|e| !e.text.is_empty())
            .collect()
    } else if let Some(text) = parsed.text {
        let text = text.trim();
        if text.is_empty() {
            vec![]
        } else {
            vec![CaptionEntry { start: range.start, end: range.end, text: text.to_string() }]
        }
    } else {
        vec![]
    };
    Ok(entries)
}

#[derive(Copy, Clone, Debug)]
struct TimeRange { start: f64, end: f64 }

fn build_overlapping_ranges(start: f64, end: f64, chunk: f64, overlap: f64) -> Vec<TimeRange> {
    let mut ranges = Vec::new();
    let safe_start = start.max(0.0);
    let safe_end = end.max(safe_start);
    if safe_end <= safe_start { return ranges }
    let step = (chunk - overlap).max(1.0);
    let mut cursor = safe_start;
    while cursor < safe_end {
        let range_end = (cursor + chunk).min(safe_end);
        ranges.push(TimeRange { start: cursor, end: range_end });
        if range_end >= safe_end { break }
        cursor += step;
    }
    ranges
}

fn dedupe_captions(mut entries: Vec<CaptionEntry>, tolerance: f64) -> Vec<CaptionEntry> {
    entries.sort_by(|a, b| {
        a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.end.partial_cmp(&b.end).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.text.cmp(&b.text))
    });
    let mut out: Vec<CaptionEntry> = Vec::with_capacity(entries.len());
    for e in entries {
        let text = e.text.trim().to_string();
        if text.is_empty() { continue }
        let normalized = CaptionEntry {
            start: e.start.max(0.0),
            end: e.end.max(e.start),
            text,
        };
        if let Some(last) = out.last_mut() {
            if last.text == normalized.text
                && (last.start - normalized.start).abs() <= tolerance
                && (last.end - normalized.end).abs() <= tolerance
            {
                last.start = last.start.min(normalized.start);
                last.end = last.end.max(normalized.end);
                continue;
            }
        }
        out.push(normalized);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_ranges_cover_the_whole_clip() {
        let r = build_overlapping_ranges(0.0, 60.0, 30.0, 0.75);
        assert_eq!(r.first().unwrap().start, 0.0);
        assert!((r.last().unwrap().end - 60.0).abs() < 1e-6);
        for w in r.windows(2) {
            assert!(w[1].start <= w[0].end, "no gap allowed");
        }
    }

    #[test]
    fn dedupe_collapses_overlap_duplicates() {
        let entries = vec![
            CaptionEntry { start: 1.00, end: 2.00, text: "hello world".into() },
            CaptionEntry { start: 1.05, end: 2.04, text: "hello world".into() },
            CaptionEntry { start: 5.00, end: 6.00, text: "next".into() },
        ];
        let out = dedupe_captions(entries, 0.08);
        assert_eq!(out.len(), 2);
    }
}

//! Parallel Claude vision analysis with a Claude aggregator merge step.
//!
//! Why parallel-then-aggregate? Each window only sees its slice of frames +
//! transcript so per-call latency is bounded by the largest window, and the
//! aggregator gets compact JSON instead of raw frames. The MVP plan flags
//! single-call vs aggregator as something to A/B once we have data; this is
//! the parallel-first implementation.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use base64::Engine;
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;

use crate::saas::SaasClient;

use super::keyframes::Keyframe;
use super::transcribe::Transcript;
use super::{ChunkKind, ResultChunk};

const MAX_FRAMES_PER_WINDOW: usize = 6;
const MAX_PARALLEL_WINDOWS: usize = 4;
/// Per-window upper bound. Normal calls return in 4–10s; anything past two
/// minutes is almost certainly a stuck upstream connection. Generous on
/// purpose so a slow-but-real call on a longer recording still lands; without
/// any cap one wedged window blocks the aggregator (and the "Analyzing…" UI
/// state) until the upstream eventually unsticks — we've seen 3+ minutes.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(120);

/// Result of the analyze stage:
/// - `final_md` — the user-facing refined prompt (streamed to the UI)
/// - `thinking_md` — a human-readable markdown rendering of the
///   intermediate per-window observations + transcript, for the
///   "Show thinking" panel.
pub struct AnalysisOutput {
    pub final_md: String,
    pub thinking_md: String,
}

pub async fn analyze_and_aggregate(
    app: AppHandle,
    id: String,
    backend: &SaasClient,
    frames: &[Keyframe],
    transcript: &Transcript,
    total_secs: f64,
    mode: &str,
) -> Result<AnalysisOutput> {
    if frames.is_empty() && transcript.entries.is_empty() {
        return Err(anyhow!("nothing to analyze — no frames or transcript"));
    }

    let windows = window_frames(frames, total_secs);
    let window_ranges: Vec<(f64, f64)> = windows.iter().map(|w| (w.t_start, w.t_end)).collect();
    let sem = Arc::new(Semaphore::new(MAX_PARALLEL_WINDOWS));

    let vision_started = Instant::now();
    let mut tasks = FuturesUnordered::new();
    for (i, w) in windows.iter().enumerate() {
        let backend = backend.clone();
        let sem = sem.clone();
        let frames = w.frames.clone();
        let slice = transcript.slice(w.t_start, w.t_end);
        let t_start = w.t_start;
        let t_end = w.t_end;
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            match tokio::time::timeout(
                WINDOW_TIMEOUT,
                analyze_window(&backend, i, t_start, t_end, &frames, &slice),
            )
            .await
            {
                Ok(res) => res,
                Err(_) => Err(anyhow!(
                    "vision window {} timed out after {}s",
                    i + 1,
                    WINDOW_TIMEOUT.as_secs()
                )),
            }
        }));
    }

    let mut window_results: Vec<(usize, Value)> = Vec::new();
    let mut failed_windows = 0usize;
    while let Some(res) = tasks.next().await {
        match res {
            Ok(Ok(v)) => window_results.push(v),
            Ok(Err(err)) => {
                failed_windows += 1;
                tracing::warn!(?err, "Claude vision window failed");
            }
            Err(err) => {
                failed_windows += 1;
                tracing::warn!(?err, "Claude vision window task panicked");
            }
        }
    }
    tracing::info!(
        elapsed_ms = vision_started.elapsed().as_millis(),
        succeeded = window_results.len(),
        failed = failed_windows,
        "stage Claude vision windows complete"
    );
    window_results.sort_by_key(|(i, _)| *i);

    if window_results.is_empty() && !frames.is_empty() {
        if transcript.entries.is_empty() {
            return Err(anyhow!(
                "all Claude vision windows failed and no transcript exists"
            ));
        }
        tracing::warn!(
            failed = failed_windows,
            "all Claude vision windows failed; continuing with transcript-only aggregation"
        );
    }

    let thinking_md = render_thinking(&window_results, &window_ranges, transcript);

    // Surface the thinking pane as early as possible — it's ready now, while
    // the aggregator still has its full streaming time ahead of it. The UI
    // shows this above the streaming prompt so a viewer of the demo can read
    // what the model "saw" before the refined prompt finishes writing.
    let _ = app.emit(
        "result:thinking",
        &json!({ "id": id, "thinking": thinking_md }),
    );

    let observations_json = serde_json::to_string_pretty(&Value::Array(
        window_results.into_iter().map(|(_, v)| v).collect(),
    ))?;

    let final_md = aggregate_streaming(
        app,
        id,
        backend,
        &observations_json,
        &transcript_summary(transcript),
        total_secs,
        mode,
    )
    .await?;

    Ok(AnalysisOutput {
        final_md,
        thinking_md,
    })
}

/// Produce a friendly markdown summary of what each per-window analyzer
/// extracted from the recording. Shown in the "Show thinking" panel so the
/// user can see why the aggregator wrote what it wrote.
fn render_thinking(
    windows: &[(usize, Value)],
    ranges: &[(f64, f64)],
    transcript: &Transcript,
) -> String {
    let mut out = String::new();
    out.push_str("## What the model saw\n\n");
    if windows.is_empty() {
        out.push_str("_(No window observations were captured.)_\n\n");
    }
    for (i, v) in windows {
        let (t_start, t_end) = ranges.get(*i).copied().unwrap_or((0.0, 0.0));
        out.push_str(&format!(
            "### Window {} — {:.1}s → {:.1}s\n\n",
            i + 1,
            t_start,
            t_end
        ));

        let speech = v
            .get("userSpeech")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !speech.is_empty() {
            out.push_str("**Heard:** ");
            out.push_str(speech);
            out.push_str("\n\n");
        }

        let pointing = v
            .get("pointing")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !pointing.is_empty() {
            out.push_str("**Pointing at:** ");
            out.push_str(pointing);
            out.push_str("\n\n");
        }

        let intent = v
            .get("actionIntent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !intent.is_empty() {
            out.push_str("**Intent:** ");
            out.push_str(intent);
            out.push_str("\n\n");
        }

        if let Some(arr) = v.get("fields").and_then(Value::as_array) {
            let mut wrote_header = false;
            for f in arr {
                let target = f.get("target").and_then(Value::as_str).unwrap_or("").trim();
                let role = f.get("role").and_then(Value::as_str).unwrap_or("").trim();
                let source = f.get("source").and_then(Value::as_str).unwrap_or("").trim();
                let example = f
                    .get("exampleValue")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if target.is_empty() && role.is_empty() {
                    continue;
                }
                if !wrote_header {
                    out.push_str("**Fields filled:**\n");
                    wrote_header = true;
                }
                out.push_str("- ");
                if !target.is_empty() {
                    out.push_str(target);
                }
                if !role.is_empty() {
                    out.push_str(" — ");
                    out.push_str(role);
                }
                if !source.is_empty() {
                    out.push_str(" (source: ");
                    out.push_str(source);
                    out.push(')');
                }
                if !example.is_empty() {
                    out.push_str(" [example: ");
                    out.push_str(example);
                    out.push(']');
                }
                out.push('\n');
            }
            if wrote_header {
                out.push('\n');
            }
        }

        if let Some(arr) = v.get("visibleContext").and_then(Value::as_array) {
            let items: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
            if !items.is_empty() {
                out.push_str("**Visible context:**\n");
                for it in items {
                    out.push_str("- ");
                    out.push_str(it);
                    out.push('\n');
                }
                out.push('\n');
            }
        }
    }

    out.push_str("## Transcript\n\n");
    if transcript.entries.is_empty() {
        out.push_str("_(No narration was captured.)_\n");
    } else {
        for e in &transcript.entries {
            out.push_str(&format!("- `{:.1}s` {}\n", e.start, e.text));
        }
    }

    out
}

#[derive(Debug)]
struct Window {
    frames: Vec<Keyframe>,
    t_start: f64,
    t_end: f64,
}

/// Bucket frames into windows of up to MAX_FRAMES_PER_WINDOW. Frame `approx_t`
/// is normalized [0,1]; we map it back into seconds using `total_secs`.
fn window_frames(frames: &[Keyframe], total_secs: f64) -> Vec<Window> {
    if frames.is_empty() {
        // Single empty window so the aggregator still gets the transcript.
        return vec![Window {
            frames: vec![],
            t_start: 0.0,
            t_end: total_secs,
        }];
    }
    let mut windows = Vec::new();
    for chunk in frames.chunks(MAX_FRAMES_PER_WINDOW) {
        let t_start = chunk
            .first()
            .map(|f| f.approx_t as f64 * total_secs)
            .unwrap_or(0.0);
        let t_end = chunk
            .last()
            .map(|f| f.approx_t as f64 * total_secs)
            .unwrap_or(total_secs);
        windows.push(Window {
            frames: chunk.to_vec(),
            t_start,
            t_end: (t_end + 1.0).min(total_secs.max(t_end + 0.5)),
        });
    }
    windows
}

async fn analyze_window(
    backend: &SaasClient,
    index: usize,
    t_start: f64,
    t_end: f64,
    frames: &[Keyframe],
    transcript_slice: &str,
) -> Result<(usize, Value)> {
    let started = Instant::now();
    let mut encoded_frames = Vec::with_capacity(frames.len());
    for frame in frames {
        encoded_frames.push(json!({
            "mediaType": "image/jpeg",
            "data": read_image_b64(&frame.path).await?,
        }));
    }
    let json_value: Value = backend
        .post_json(
            "/api/vision-window",
            json!({
                "index": index,
                "tStart": t_start,
                "tEnd": t_end,
                "frames": encoded_frames,
                "transcriptSlice": transcript_slice,
            }),
        )
        .await?;
    tracing::info!(
        window = index + 1,
        elapsed_ms = started.elapsed().as_millis(),
        "stage Peer backend vision window complete"
    );
    Ok((index, json_value))
}

fn transcript_summary(t: &Transcript) -> String {
    if t.entries.is_empty() {
        return "(no narration captured)".into();
    }
    t.entries
        .iter()
        .map(|e| format!("[{:.1}s] {}", e.start, e.text))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn read_image_b64(path: &Path) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

async fn aggregate_streaming(
    app: AppHandle,
    id: String,
    backend: &SaasClient,
    observations_json: &str,
    transcript_text: &str,
    total_secs: f64,
    mode: &str,
) -> Result<String> {
    let started = Instant::now();
    let res = backend
        .post_stream("/api/aggregate")?
        .json(&json!({
            "observationsJson": observations_json,
            "transcriptText": transcript_text,
            "totalSecs": total_secs,
            "mode": mode,
        }))
        .send()
        .await?;

    if !res.status().is_success() {
        let s = res.status();
        let t = res.text().await.unwrap_or_default();
        return Err(anyhow!("Peer backend aggregator: {s} — {t}"));
    }

    let mut acc = String::new();
    let mut stream = res.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        loop {
            let Some(idx) = buf.find("\n\n") else { break };
            let event_block = buf[..idx].to_string();
            buf.drain(..=idx + 1);

            for line in event_block.lines() {
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v): Result<Value, _> = serde_json::from_str(data) else {
                    continue;
                };
                let kind = v["type"].as_str().unwrap_or("");
                if kind == "content_block_delta" {
                    if let Some(text) = v["delta"]["text"].as_str() {
                        acc.push_str(text);
                        let chunk = ResultChunk {
                            id: id.clone(),
                            kind: ChunkKind::Delta,
                            text: text.to_string(),
                        };
                        let _ = app.emit("result:chunk", &chunk);
                    }
                }
            }
        }
    }

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        output_chars = acc.len(),
        "stage Peer backend aggregation complete"
    );
    Ok(acc.trim().to_string())
}

//! Parallel Claude analysis with an aggregator merge step.
//!
//! Why parallel-then-aggregate? Each window only sees its slice of frames +
//! transcript so per-call latency is bounded by the largest window, and the
//! aggregator gets compact JSON instead of raw frames. The MVP plan flags
//! single-call vs aggregator as something to A/B once we have data; this is
//! the parallel-first implementation.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;

use super::keyframes::Keyframe;
use super::prompts;
use super::transcribe::Transcript;
use super::{ChunkKind, ResultChunk};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const WINDOW_MODEL: &str = "claude-sonnet-4-6";
const AGGREGATOR_MODEL: &str = "claude-sonnet-4-6";
const MAX_FRAMES_PER_WINDOW: usize = 6;
const MAX_PARALLEL_WINDOWS: usize = 4;

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
    anthropic_key: &str,
    frames: &[Keyframe],
    transcript: &Transcript,
    total_secs: f64,
) -> Result<AnalysisOutput> {
    if frames.is_empty() && transcript.entries.is_empty() {
        return Err(anyhow!("nothing to analyze — no frames or transcript"));
    }

    let windows = window_frames(frames, total_secs);
    let window_ranges: Vec<(f64, f64)> = windows.iter().map(|w| (w.t_start, w.t_end)).collect();
    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?,
    );
    let sem = Arc::new(Semaphore::new(MAX_PARALLEL_WINDOWS));

    let mut tasks = FuturesUnordered::new();
    for (i, w) in windows.iter().enumerate() {
        let client = client.clone();
        let key = anthropic_key.to_string();
        let sem = sem.clone();
        let frames = w.frames.clone();
        let slice = transcript.slice(w.t_start, w.t_end);
        let t_start = w.t_start;
        let t_end = w.t_end;
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            analyze_window(&client, &key, i, t_start, t_end, &frames, &slice).await
        }));
    }

    let mut window_results: Vec<(usize, Value)> = Vec::new();
    while let Some(res) = tasks.next().await {
        match res {
            Ok(Ok(v)) => window_results.push(v),
            Ok(Err(err)) => tracing::warn!(?err, "window analysis failed"),
            Err(err) => tracing::warn!(?err, "window task panicked"),
        }
    }
    window_results.sort_by_key(|(i, _)| *i);

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
        &client,
        anthropic_key,
        &observations_json,
        &transcript_summary(transcript),
        total_secs,
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
    client: &reqwest::Client,
    key: &str,
    index: usize,
    t_start: f64,
    t_end: f64,
    frames: &[Keyframe],
    transcript_slice: &str,
) -> Result<(usize, Value)> {
    let mut content: Vec<Value> = Vec::with_capacity(frames.len() + 1);
    for frame in frames {
        let data = read_image_b64(&frame.path).await?;
        content.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/jpeg",
                "data": data,
            }
        }));
    }
    content.push(json!({
        "type": "text",
        "text": format!(
            "Window {idx} — covers t≈{ts:.1}s..t≈{te:.1}s of the recording.\n\nNarration in this window:\n{tx}\n\nReturn the JSON object now.",
            idx = index + 1,
            ts = t_start,
            te = t_end,
            tx = if transcript_slice.is_empty() { "(no narration in this window)".into() } else { transcript_slice.to_string() },
        ),
    }));

    let body = json!({
        "model": WINDOW_MODEL,
        "max_tokens": 1024,
        "system": [{
            "type": "text",
            "text": prompts::WINDOW_SYSTEM,
            "cache_control": { "type": "ephemeral" }
        }],
        "messages": [{ "role": "user", "content": content }],
    });

    let res = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !res.status().is_success() {
        let s = res.status();
        let t = res.text().await.unwrap_or_default();
        return Err(anyhow!("Claude window {idx}: {s} — {t}", idx = index));
    }
    let payload: Value = res.json().await?;
    let text = payload["content"]
        .as_array()
        .and_then(|a| a.iter().find_map(|v| v["text"].as_str()))
        .unwrap_or_default()
        .trim()
        .to_string();

    let json_value = extract_json(&text)
        .unwrap_or_else(|| json!({ "userSpeech": text, "visibleContext": [], "pointing": "" }));
    Ok((index, json_value))
}

fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    // Strip fenced ```json blocks
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        if let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
            return Some(v);
        }
    }
    None
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
    client: &reqwest::Client,
    key: &str,
    observations_json: &str,
    transcript_text: &str,
    total_secs: f64,
) -> Result<String> {
    let user_msg = format!(
        "Recording duration: {:.1}s\n\nFull transcript (with timestamps):\n{}\n\nPer-window notes (JSON, ordered):\n{}\n\nNow produce the refined prompt per the system prompt.",
        total_secs, transcript_text, observations_json,
    );

    let body = json!({
        "model": AGGREGATOR_MODEL,
        "max_tokens": 2048,
        "stream": true,
        "system": [{
            "type": "text",
            "text": prompts::AGGREGATOR_SYSTEM,
            "cache_control": { "type": "ephemeral" }
        }],
        "messages": [{ "role": "user", "content": [{"type":"text","text": user_msg }] }],
    });

    let res = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await?;

    if !res.status().is_success() {
        let s = res.status();
        let t = res.text().await.unwrap_or_default();
        return Err(anyhow!("aggregator: {s} — {t}"));
    }

    let mut acc = String::new();
    let mut stream = res.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        // Process complete SSE events ending in \n\n
        loop {
            let Some(idx) = buf.find("\n\n") else { break };
            let event_block = buf[..idx].to_string();
            buf.drain(..=idx + 1); // drop block + trailing \n\n

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
    Ok(acc.trim().to_string())
}

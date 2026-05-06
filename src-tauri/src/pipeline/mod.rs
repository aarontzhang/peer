//! Orchestrator: video → keyframes + transcript (parallel) → analysis (parallel) → aggregate.

mod analyze;
mod ffprobe;
mod keyframes;
mod prompts;
mod transcribe;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::db::RecordingStatus;
use crate::ipc;
use crate::recording::{emit, PillEvent};
use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResultChunk {
    pub id: String,
    pub kind: ChunkKind,
    pub text: String,
}

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ChunkKind {
    Begin,
    Delta,
    End,
}

pub async fn run(
    app: AppHandle,
    state: Arc<AppState>,
    id: String,
    video_path: PathBuf,
    duration_ms: u64,
) -> Result<()> {
    let openai =
        ipc::read_api_key(&app, "openai").context("OpenAI key missing — set it in Settings")?;
    let anthropic = ipc::read_api_key(&app, "anthropic")
        .context("Anthropic key missing — set it in Settings")?;

    // Mark processing.
    if let Some(mut rec) = state.db().get_recording(&id).await? {
        rec.status = RecordingStatus::Processing;
        rec.duration_ms = duration_ms;
        state.db().update_recording(&rec).await?;
    }

    let probe = ffprobe::probe(&video_path).await?;
    tracing::info!(?probe, "probed");
    let total_secs = probe.duration_secs.max(duration_ms as f64 / 1000.0);

    // Parallel: keyframes + transcript
    let frames_dir = state.frames_dir.join(&id);
    tokio::fs::create_dir_all(&frames_dir).await.ok();

    let app_kf = app.clone();
    let id_kf = id.clone();
    let video_kf = video_path.clone();
    let frames_kf = frames_dir.clone();
    let kf_handle = tokio::spawn(async move {
        emit(
            &app_kf,
            &PillEvent::Processing {
                id: id_kf.clone(),
                label: "Extracting keyframes".into(),
                progress: 0.18,
            },
        );
        keyframes::extract(&video_kf, &frames_kf).await
    });

    let app_tx = app.clone();
    let id_tx = id.clone();
    let video_tx = video_path.clone();
    let openai_clone = openai.clone();
    let tx_handle = tokio::spawn(async move {
        emit(
            &app_tx,
            &PillEvent::Processing {
                id: id_tx.clone(),
                label: "Transcribing audio".into(),
                progress: 0.22,
            },
        );
        transcribe::transcribe(&video_tx, &openai_clone, total_secs).await
    });

    let frames = kf_handle.await??;
    emit(
        &app,
        &PillEvent::Processing {
            id: id.clone(),
            label: "Keyframes ready".into(),
            progress: 0.45,
        },
    );
    let transcript = tx_handle.await??;
    emit(
        &app,
        &PillEvent::Processing {
            id: id.clone(),
            label: "Transcript ready".into(),
            progress: 0.62,
        },
    );

    // Persist transcript text up front so the result UI can show it even
    // before the analyzer finishes.
    if let Some(mut rec) = state.db().get_recording(&id).await? {
        rec.transcript = Some(transcript.plain_text());
        state.db().update_recording(&rec).await?;
    }

    let _ = app.emit(
        "result:transcript",
        &serde_json::json!({
            "id": id,
            "transcript": transcript.plain_text(),
        }),
    );

    emit(
        &app,
        &PillEvent::Processing {
            id: id.clone(),
            label: "Analyzing".into(),
            progress: 0.7,
        },
    );

    let begin = ResultChunk {
        id: id.clone(),
        kind: ChunkKind::Begin,
        text: String::new(),
    };
    let _ = app.emit("result:chunk", &begin);

    let analyze::AnalysisOutput {
        final_md,
        thinking_md,
    } = analyze::analyze_and_aggregate(
        app.clone(),
        id.clone(),
        &anthropic,
        &frames,
        &transcript,
        total_secs,
    )
    .await?;

    let final_text = normalize_prompt_text(&final_md);
    let summary = first_line(&final_text);

    if let Some(mut rec) = state.db().get_recording(&id).await? {
        rec.status = RecordingStatus::Done;
        rec.summary = Some(summary);
        rec.body = Some(final_text.clone());
        rec.thinking = Some(thinking_md);
        state.db().update_recording(&rec).await?;
    }

    let end = ResultChunk {
        id: id.clone(),
        kind: ChunkKind::End,
        text: final_text.clone(),
    };
    let _ = app.emit("result:chunk", &end);

    // Auto-copy.
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let _ = app.clipboard().write_text(final_text);

    Ok(())
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn normalize_prompt_text(s: &str) -> String {
    let mut text = s.replace("\r\n", "\n").replace('\r', "\n");
    text = strip_fenced_markers(&text, "```");
    text = strip_fenced_markers(&text, "~~~");
    text = text
        .lines()
        .map(strip_markdown_line_prefixes)
        .collect::<Vec<_>>()
        .join("\n");

    for delim in ["**", "__", "~~", "`", "*"] {
        text = strip_wrapped_segments(&text, delim);
    }

    let text = strip_markdown_links(&text);
    collapse_blank_lines(&text).trim().to_string()
}

fn strip_fenced_markers(input: &str, fence: &str) -> String {
    input
        .lines()
        .filter(|line| !line.trim_start().starts_with(fence))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_markdown_line_prefixes(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return String::new();
    }
    if is_horizontal_rule(trimmed) {
        return String::new();
    }

    let mut s = trimmed;
    if let Some(rest) = s.strip_prefix('>') {
        s = rest.trim_start();
    }
    s = s.trim_start_matches('#').trim_start();
    s = strip_list_prefix(s);
    if let Some(rest) = s
        .strip_prefix("[ ] ")
        .or_else(|| s.strip_prefix("[x] "))
        .or_else(|| s.strip_prefix("[X] "))
    {
        s = rest;
    }
    s.trim_end().to_string()
}

fn is_horizontal_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3 && compact.chars().all(|c| matches!(c, '-' | '*' | '_'))
}

fn strip_list_prefix(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix("- ") {
        return rest;
    }
    if let Some(rest) = line.strip_prefix("* ") {
        return rest;
    }
    if let Some(rest) = line.strip_prefix("+ ") {
        return rest;
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest;
        }
    }
    line
}

fn strip_wrapped_segments(input: &str, delim: &str) -> String {
    if delim.is_empty() {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find(delim) {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + delim.len()..];
        let Some(end) = after_start.find(delim) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after_start[..end];
        if inner.trim().is_empty() || inner.contains('\n') {
            out.push_str(delim);
            rest = after_start;
            continue;
        }
        out.push_str(inner);
        rest = &after_start[end + delim.len()..];
    }

    out.push_str(rest);
    out
}

fn strip_markdown_links(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let mut last = 0usize;

    while cursor < input.len() {
        let rest = &input[cursor..];
        if rest.starts_with("![") {
            if let Some((label, next)) = parse_link(input, cursor + 1) {
                out.push_str(&input[last..cursor]);
                out.push_str(label);
                cursor = next;
                last = next;
                continue;
            }
        }
        if rest.starts_with('[') {
            if let Some((label, next)) = parse_link(input, cursor) {
                out.push_str(&input[last..cursor]);
                out.push_str(label);
                cursor = next;
                last = next;
                continue;
            }
        }
        cursor += rest.chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
    }

    out.push_str(&input[last..]);
    out
}

fn parse_link(input: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = input.as_bytes();
    let close = bytes[start + 1..]
        .iter()
        .position(|&b| b == b']')
        .map(|idx| start + 1 + idx)?;
    if bytes.get(close + 1) != Some(&b'(') {
        return None;
    }
    let end = bytes[close + 2..]
        .iter()
        .position(|&b| b == b')')
        .map(|idx| close + 2 + idx)?;
    let label = &input[start + 1..close];
    Some((label, end + 1))
}

fn collapse_blank_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut blank_run = 0usize;
    for line in input.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Pull the last few non-empty lines of an ffmpeg/ffprobe stderr buffer so a
/// surfaced error doesn't include the entire build banner.
pub(crate) fn tail_stderr(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let last: Vec<&str> = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(3)
        .collect();
    let mut out: Vec<&str> = last.into_iter().rev().collect();
    if out.is_empty() {
        out.push("no output");
    }
    out.join(" | ")
}

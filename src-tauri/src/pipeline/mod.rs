//! Orchestrator: video → keyframes + transcript (parallel) → analysis (parallel) → aggregate.

mod analyze;
mod ffprobe;
mod keyframes;
mod transcribe;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::db::{RecordingStatus, VersionSource};
use crate::recording::{emit, PillEvent};
use crate::saas::SaasClient;
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
    version_source: VersionSource,
) -> Result<()> {
    let total_started = Instant::now();
    let saas = SaasClient::from_keychain(app.clone())
        .await
        .context("Sign in to use Peer — recording requires a Peer account")?;

    // Mark processing.
    if let Some(mut rec) = state.db().get_recording(&id).await? {
        rec.status = RecordingStatus::Processing;
        rec.duration_ms = duration_ms;
        state.db().update_recording(&rec).await?;
    }

    let probe_started = Instant::now();
    let probe = ffprobe::probe(&video_path).await?;
    tracing::info!(
        elapsed_ms = probe_started.elapsed().as_millis(),
        "stage probe complete"
    );
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
        let started = Instant::now();
        emit(
            &app_kf,
            &PillEvent::Processing {
                id: id_kf.clone(),
                label: "Extracting keyframes".into(),
                progress: 0.18,
            },
        );
        let result = keyframes::extract(&video_kf, &frames_kf).await;
        match &result {
            Ok(frames) => tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                frames = frames.len(),
                "stage keyframe extraction complete"
            ),
            Err(err) => tracing::warn!(
                elapsed_ms = started.elapsed().as_millis(),
                ?err,
                "stage keyframe extraction failed"
            ),
        }
        result
    });

    let app_tx = app.clone();
    let id_tx = id.clone();
    let video_tx = video_path.clone();
    let saas_tx = saas.clone();
    let tx_handle = tokio::spawn(async move {
        let started = Instant::now();
        emit(
            &app_tx,
            &PillEvent::Processing {
                id: id_tx.clone(),
                label: "Transcribing audio".into(),
                progress: 0.22,
            },
        );
        let result = transcribe::transcribe(&video_tx, &saas_tx, total_secs).await;
        match &result {
            Ok(transcript) => tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                entries = transcript.entries.len(),
                "stage audio extraction/transcription complete"
            ),
            Err(err) => tracing::warn!(
                elapsed_ms = started.elapsed().as_millis(),
                ?err,
                "stage audio extraction/transcription failed"
            ),
        }
        result
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

    let mode = state.permission_mode.lock().as_str();
    let analyze::AnalysisOutput {
        final_md,
        thinking_md,
    } = analyze::analyze_and_aggregate(
        app.clone(),
        id.clone(),
        &saas,
        &frames,
        &transcript,
        total_secs,
        mode,
    )
    .await?;

    let final_text = normalize_prompt_text(&final_md);
    let fallback_summary = first_line(&final_text);

    // Persist the prompt body and a provisional summary immediately so the
    // sidebar updates as soon as the stream ends. The LLM-generated title
    // arrives in a follow-up update below — a brief flicker from first-line
    // to title is fine; making the user wait on Haiku before seeing the
    // result row settle is not.
    //
    // `append_version` writes the new body AND a `recording_versions` row in
    // one transaction so the timeline is never out of sync with `body`.
    state
        .db()
        .append_version(
            &id,
            version_source,
            &final_text,
            Some(&thinking_md),
            None,
            None,
        )
        .await?;
    if let Some(mut rec) = state.db().get_recording(&id).await? {
        rec.status = RecordingStatus::Done;
        rec.summary = Some(fallback_summary.clone());
        state.db().update_recording(&rec).await?;
    }

    let end = ResultChunk {
        id: id.clone(),
        kind: ChunkKind::End,
        text: final_text.clone(),
    };
    let _ = app.emit("result:chunk", &end);

    // Codex-style short title via Haiku. The PillEvent::Done emitted after
    // run() returns triggers the frontend to refresh the sidebar, so the
    // updated summary is picked up without an extra event.
    let title = match generate_title(&saas, &final_text).await {
        Ok(t) if !t.is_empty() => t,
        Ok(_) => {
            tracing::warn!("title generation returned empty string; using first line");
            fallback_summary.clone()
        }
        Err(err) => {
            tracing::warn!(?err, "title generation failed; using first line");
            fallback_summary.clone()
        }
    };
    if title != fallback_summary {
        if let Some(mut rec) = state.db().get_recording(&id).await? {
            rec.summary = Some(title);
            state.db().update_recording(&rec).await?;
        }
    }

    // Auto-copy.
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let _ = app.clipboard().write_text(final_text);

    tracing::info!(
        elapsed_ms = total_started.elapsed().as_millis(),
        "stage total processing complete"
    );

    Ok(())
}

#[derive(Deserialize)]
struct TitleResponse {
    title: Option<String>,
}

/// Ask the backend (Haiku) for a Codex-style 3-5 word title summarizing the
/// finished prompt. Returns the trimmed title, or an error so the caller can
/// fall back to `first_line`. Anything longer than ~12 words gets rejected
/// since the row is single-line; better to fall back than to truncate awkwardly.
async fn generate_title(saas: &SaasClient, prompt: &str) -> Result<String> {
    let started = Instant::now();
    let res: TitleResponse = saas
        .post_json("/api/title", json!({ "prompt": prompt }))
        .await?;
    let title = res
        .title
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        chars = title.len(),
        "stage Peer backend title complete"
    );
    if title.split_whitespace().count() > 12 {
        return Err(anyhow::anyhow!("title too long: {title:?}"));
    }
    Ok(title)
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

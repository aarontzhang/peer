//! Chat refinement of a previously generated prompt.
//!
//! Wire shape mirrors `pipeline::analyze::aggregate_streaming`: POST to the
//! Peer backend with structured fields, stream SSE deltas back, and emit
//! `chat:chunk` events to the frontend so the dock bubble and the live
//! prompt body can update simultaneously.
//!
//! The Rust side sends structured fields (currentBody, transcript,
//! observationsJson, thread, newMessage). The backend is responsible for
//! wrapping each in `<…>` tags before composing the Claude user message
//! and for instructing Claude that tagged content is data, not
//! instructions. Keeping that responsibility in one place means the
//! injection-defense system prompt only needs to live in one repo.

use std::time::Instant;

use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::db::ChatRole;
use crate::saas::SaasClient;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatChunk {
    pub recording_id: String,
    pub turn_id: String,
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

#[derive(Debug, Deserialize)]
struct ChatErrorBody {
    #[serde(default)]
    message: Option<String>,
}

/// Inputs needed to refine a prompt by chat. The prompt body already
/// encodes everything the model needs about the original recording — we
/// don't re-send transcript or vision observations on every turn (cost +
/// latency), only when the user explicitly hits Retry (which re-runs the
/// full pipeline). `thread` is the prior turns in chronological order.
pub struct ChatRequest<'a> {
    pub recording_id: &'a str,
    pub turn_id: &'a str,
    pub current_body: &'a str,
    pub mode: &'a str,
    pub thread: &'a [ChatThreadEntry],
    pub new_message: &'a str,
}

/// Owned variant of `ThreadEntry` so it can outlive a single async call
/// frame (the caller fetches the thread from the DB before spawning a
/// background streamer).
#[derive(Debug, Clone)]
pub struct ChatThreadEntry {
    pub role: ChatRole,
    pub content: String,
}

impl ChatThreadEntry {
    fn role_str(&self) -> &'static str {
        match self.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

/// Stream a chat-refinement turn from the Peer backend. Emits a `chat:chunk`
/// Begin → many Delta → End sequence (mirrors `result:chunk` for initial
/// generation). Returns the accumulated assistant text on success.
pub async fn stream_chat_turn(
    app: AppHandle,
    backend: &SaasClient,
    req: ChatRequest<'_>,
) -> Result<String> {
    let started = Instant::now();

    let begin = ChatChunk {
        recording_id: req.recording_id.to_string(),
        turn_id: req.turn_id.to_string(),
        kind: ChunkKind::Begin,
        text: String::new(),
    };
    let _ = app.emit("chat:chunk", &begin);

    let thread: Vec<Value> = req
        .thread
        .iter()
        .map(|e| json!({ "role": e.role_str(), "content": e.content }))
        .collect();

    let res = backend
        .post_stream("/api/chat")?
        .json(&json!({
            "currentBody": req.current_body,
            "mode": req.mode,
            "thread": thread,
            "newMessage": req.new_message,
        }))
        .send()
        .await?;

    if !res.status().is_success() {
        let s = res.status();
        let body = res.text().await.unwrap_or_default();
        // Try to surface a server-supplied human message rather than the raw
        // body, which is often Vercel's HTML 500 page.
        let detail = serde_json::from_str::<ChatErrorBody>(&body)
            .ok()
            .and_then(|b| b.message)
            .unwrap_or(body);
        return Err(anyhow!("Peer backend chat: {s} — {detail}"));
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
                if v["type"].as_str() == Some("content_block_delta") {
                    if let Some(text) = v["delta"]["text"].as_str() {
                        acc.push_str(text);
                        let delta = ChatChunk {
                            recording_id: req.recording_id.to_string(),
                            turn_id: req.turn_id.to_string(),
                            kind: ChunkKind::Delta,
                            text: text.to_string(),
                        };
                        let _ = app.emit("chat:chunk", &delta);
                    }
                }
            }
        }
    }

    let end = ChatChunk {
        recording_id: req.recording_id.to_string(),
        turn_id: req.turn_id.to_string(),
        kind: ChunkKind::End,
        text: acc.clone(),
    };
    let _ = app.emit("chat:chunk", &end);

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        output_chars = acc.len(),
        "stage chat refinement complete"
    );
    Ok(acc.trim().to_string())
}

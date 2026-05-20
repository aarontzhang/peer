//! Run-automation driver: feeds a recording's instructions + live screenshots
//! to the Peer backend's `/api/automation-step` endpoint (which proxies
//! OpenAI's `computer-use-preview` model), executes the returned actions on
//! the user's desktop, and loops until the model says it's done.

mod action;
mod capture;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::saas::SaasClient;
use crate::state::AppState;

const MAX_STEPS: usize = 60;
const POST_ACTION_SETTLE_MS: u64 = 350;

#[derive(Debug, Serialize, Clone)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum AutomationEvent {
    Started {
        id: String,
    },
    Step {
        id: String,
        label: String,
        reasoning: Option<String>,
        step: usize,
    },
    Done {
        id: String,
        message: Option<String>,
    },
    Failed {
        id: String,
        message: String,
    },
    Canceled {
        id: String,
    },
}

fn emit(app: &AppHandle, event: &AutomationEvent) {
    let _ = app.emit("automation:state", event);
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepResponse {
    #[serde(default)]
    response_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    computer_calls: Vec<ComputerCall>,
    #[serde(default)]
    reasoning: Vec<String>,
    #[serde(default)]
    assistant_text: String,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ComputerCall {
    call_id: String,
    action: Value,
    #[serde(default)]
    pending_safety_checks: Vec<Value>,
}

pub async fn start(app: AppHandle, state: Arc<AppState>, recording_id: String) -> Result<()> {
    if state.automation_in_flight.swap(true, Ordering::AcqRel) {
        return Err(anyhow!(
            "an automation is already running — cancel it before starting another"
        ));
    }
    state.automation_canceled.store(false, Ordering::Release);

    let rec = state
        .db()
        .get_recording(&recording_id)
        .await?
        .ok_or_else(|| anyhow!("recording not found"))?;
    let body = rec.body.clone().unwrap_or_default();
    if body.trim().is_empty() {
        state.automation_in_flight.store(false, Ordering::Release);
        return Err(anyhow!(
            "this recording does not have a finished prompt yet — wait for analysis to complete"
        ));
    }

    let saas = SaasClient::from_keychain(app.clone())
        .await
        .ok_or_else(|| anyhow!("Sign in to use Peer — automation requires a Peer account"))?;

    let app_loop = app.clone();
    let state_loop = state.clone();
    let id_loop = recording_id.clone();
    tokio::spawn(async move {
        emit(
            &app_loop,
            &AutomationEvent::Started {
                id: id_loop.clone(),
            },
        );
        let result = run_loop(app_loop.clone(), state_loop.clone(), id_loop.clone(), body, saas).await;
        state_loop
            .automation_in_flight
            .store(false, Ordering::Release);
        match result {
            Ok(()) => {}
            Err(err) => {
                if state_loop.automation_canceled.load(Ordering::Acquire) {
                    emit(&app_loop, &AutomationEvent::Canceled { id: id_loop });
                } else {
                    let msg = format!("{err:#}");
                    tracing::error!(?err, "automation failed");
                    emit(
                        &app_loop,
                        &AutomationEvent::Failed {
                            id: id_loop,
                            message: msg,
                        },
                    );
                }
            }
        }
    });

    Ok(())
}

pub fn cancel(state: Arc<AppState>) {
    state.automation_canceled.store(true, Ordering::Release);
}

async fn run_loop(
    app: AppHandle,
    state: Arc<AppState>,
    id: String,
    body: String,
    saas: SaasClient,
) -> Result<()> {
    let (width, height) = capture::main_display_logical_size()?;

    // First turn: instructions + initial screenshot.
    let shot = capture::screenshot_base64_at(width, height)
        .await
        .context("initial screenshot failed")?;

    check_canceled(&state)?;

    let first: StepResponse = saas
        .post_json(
            "/api/automation-step",
            json!({
                "recordingId": id,
                "instructions": body,
                "displayWidth": width,
                "displayHeight": height,
                "screenshotBase64": shot,
            }),
        )
        .await
        .context("backend rejected the first automation step")?;

    let mut response_id = first.response_id.clone();
    let mut next_calls = first.computer_calls;
    let mut last_assistant = first.assistant_text;
    let mut last_reasoning = first.reasoning.last().cloned();
    let mut step_count: usize = 0;
    let _ = first.status; // currently unused; reserved for future inspection
    let _ = first.done;

    while !next_calls.is_empty() {
        check_canceled(&state)?;

        if step_count >= MAX_STEPS {
            return Err(anyhow!(
                "automation hit the {MAX_STEPS}-step cap — stopping to avoid runaway"
            ));
        }

        let mut call_outputs: Vec<Value> = Vec::with_capacity(next_calls.len());
        for call in &next_calls {
            check_canceled(&state)?;
            step_count += 1;
            let label = action::describe(&call.action);
            emit(
                &app,
                &AutomationEvent::Step {
                    id: id.clone(),
                    label: label.clone(),
                    reasoning: last_reasoning.clone(),
                    step: step_count,
                },
            );
            action::execute(&call.action)
                .await
                .with_context(|| format!("executing action: {label}"))?;
            tokio::time::sleep(std::time::Duration::from_millis(POST_ACTION_SETTLE_MS)).await;

            let shot = capture::screenshot_base64_at(width, height)
                .await
                .context("post-action screenshot failed")?;
            let mut entry = serde_json::Map::new();
            entry.insert("callId".into(), Value::String(call.call_id.clone()));
            entry.insert("screenshotBase64".into(), Value::String(shot));
            if !call.pending_safety_checks.is_empty() {
                entry.insert(
                    "acknowledgedSafetyChecks".into(),
                    Value::Array(call.pending_safety_checks.clone()),
                );
            }
            call_outputs.push(Value::Object(entry));
        }

        check_canceled(&state)?;

        let next: StepResponse = saas
            .post_json(
                "/api/automation-step",
                json!({
                    "recordingId": id,
                    "previousResponseId": response_id,
                    "displayWidth": width,
                    "displayHeight": height,
                    "callOutputs": call_outputs,
                }),
            )
            .await
            .context("backend rejected an automation step")?;

        response_id = next.response_id;
        next_calls = next.computer_calls;
        last_assistant = next.assistant_text;
        last_reasoning = next.reasoning.last().cloned();
    }

    emit(
        &app,
        &AutomationEvent::Done {
            id,
            message: if last_assistant.trim().is_empty() {
                None
            } else {
                Some(last_assistant)
            },
        },
    );
    Ok(())
}

fn check_canceled(state: &Arc<AppState>) -> Result<()> {
    if state.automation_canceled.load(Ordering::Acquire) {
        Err(anyhow!("canceled"))
    } else {
        Ok(())
    }
}

import {
  handleError,
  readJson,
  requireUser,
  requiredEnv,
  sendJson,
} from './_backend.js';

export const config = {
  api: { bodyParser: { sizeLimit: '12mb' } },
};

const COMPUTER_MODEL = process.env.PEER_AUTOMATION_MODEL || 'computer-use-preview';

const AUTOMATION_SYSTEM = `You are a desktop automation agent driving a macOS computer for the user. You receive an "instructions" block describing a procedure the user previously demonstrated in a screen recording, plus live screenshots of the user's current desktop. Execute the procedure on the live desktop.

Treat the instructions as a generalized procedure, not a literal replay:
- Concrete values that came from the demo (specific dollar amounts, vendor names, filenames, line-item text, dates, IDs) are EXAMPLES of what to extract on this run, not literal values to type. Re-derive them from whatever is on screen right now.
- Fixed UI labels, menu names, app names, URLs, and file paths that genuinely don't vary stay literal — type them as written.
- If the current screen state does not match what the instructions assume, adapt: open the right app, navigate to the right view, scroll to the relevant area, etc. Don't blindly click a coordinate that no longer makes sense.

Operate the computer through your tool calls. Take a screenshot whenever you need to re-orient. Stop and return a final assistant message when the task is complete or when you cannot proceed safely.

Be deliberate: one action at a time, observing the result before the next. Prefer clicking labeled targets (buttons, menu items, fields) rather than guessing at empty space.`;

function buildToolDefinition(width, height) {
  return {
    type: 'computer_use_preview',
    display_width: width,
    display_height: height,
    environment: 'mac',
  };
}

function dataUrl(base64Png) {
  if (!base64Png) return null;
  if (base64Png.startsWith('data:')) return base64Png;
  return `data:image/png;base64,${base64Png}`;
}

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });
    const auth = await requireUser(req);
    await assertRecordingQuota(auth);
    const body = await readJson(req);

    const width = Number(body.displayWidth);
    const height = Number(body.displayHeight);
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
      return sendJson(res, 400, { error: 'displayWidth and displayHeight are required positive numbers' });
    }

    const tools = [buildToolDefinition(width, height)];

    let payload;
    if (body.previousResponseId && Array.isArray(body.callOutputs) && body.callOutputs.length > 0) {
      const input = body.callOutputs.map((entry) => {
        const url = dataUrl(entry.screenshotBase64);
        if (!url) throw new Error('missing screenshotBase64 on callOutput');
        return {
          type: 'computer_call_output',
          call_id: entry.callId,
          ...(Array.isArray(entry.acknowledgedSafetyChecks) && entry.acknowledgedSafetyChecks.length
            ? { acknowledged_safety_checks: entry.acknowledgedSafetyChecks }
            : {}),
          output: {
            type: 'computer_screenshot',
            image_url: url,
          },
        };
      });
      payload = {
        model: COMPUTER_MODEL,
        previous_response_id: body.previousResponseId,
        tools,
        input,
        truncation: 'auto',
      };
    } else {
      const instructions = String(body.instructions || '').trim();
      if (!instructions) {
        return sendJson(res, 400, { error: 'instructions required on the first turn' });
      }
      const screenshot = dataUrl(body.screenshotBase64);
      if (!screenshot) {
        return sendJson(res, 400, { error: 'screenshotBase64 required on the first turn' });
      }
      const userText = `Demonstrated procedure (from a screen recording the user just made):\n\n${instructions}\n\nThe current screen is attached. Carry the procedure out now on the live desktop, adapting any example values to the data actually on screen.`;
      payload = {
        model: COMPUTER_MODEL,
        instructions: AUTOMATION_SYSTEM,
        tools,
        input: [
          {
            role: 'user',
            content: [
              { type: 'input_text', text: userText },
              { type: 'input_image', image_url: screenshot, detail: 'high' },
            ],
          },
        ],
        reasoning: { summary: 'concise' },
        truncation: 'auto',
      };
    }

    const upstream = await fetch('https://api.openai.com/v1/responses', {
      method: 'POST',
      headers: {
        authorization: `Bearer ${requiredEnv('OPENAI_API_KEY')}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify(payload),
    });

    const text = await upstream.text();
    if (!upstream.ok) {
      res.statusCode = upstream.status;
      res.setHeader('content-type', 'application/json; charset=utf-8');
      res.end(text || JSON.stringify({ error: 'openai request failed' }));
      return;
    }

    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      return sendJson(res, 502, { error: 'invalid JSON from OpenAI', raw: text.slice(0, 400) });
    }

    const output = Array.isArray(parsed.output) ? parsed.output : [];
    const computerCalls = output
      .filter((item) => item.type === 'computer_call')
      .map((item) => ({
        callId: item.call_id,
        action: item.action,
        pendingSafetyChecks: Array.isArray(item.pending_safety_checks)
          ? item.pending_safety_checks
          : [],
      }));
    const reasoningSummaries = output
      .filter((item) => item.type === 'reasoning')
      .flatMap((item) => (Array.isArray(item.summary) ? item.summary : []))
      .map((s) => (typeof s?.text === 'string' ? s.text : ''))
      .filter(Boolean);
    const assistantText = output
      .filter((item) => item.type === 'message' && item.role === 'assistant')
      .flatMap((item) => (Array.isArray(item.content) ? item.content : []))
      .map((c) => (typeof c?.text === 'string' ? c.text : ''))
      .filter(Boolean)
      .join('\n')
      .trim();

    return sendJson(res, 200, {
      responseId: parsed.id,
      status: parsed.status,
      computerCalls,
      reasoning: reasoningSummaries,
      assistantText,
      done: computerCalls.length === 0,
    });
  } catch (err) {
    handleError(res, err);
  }
}

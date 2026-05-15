import {
  AGGREGATOR_SYSTEM,
  MODE_ASK_SUFFIX,
  MODE_BYPASS_SUFFIX,
  assertRecordingQuota,
  handleError,
  readJson,
  recordUsage,
  requireUser,
  requiredEnv,
  sendJson,
} from './_backend.js';

export const config = {
  api: { bodyParser: { sizeLimit: '8mb' } },
};

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });
    const auth = await requireUser(req);
    await assertRecordingQuota(auth);
    const body = await readJson(req);

    const userMessage = `Recording duration: ${Number(body.totalSecs || 0).toFixed(1)}s\n\nFull transcript (with timestamps):\n${body.transcriptText || '(no narration captured)'}\n\nPer-window notes (JSON, ordered):\n${body.observationsJson || '[]'}\n\nNow produce the refined prompt per the system prompt.`;
    const modeSuffix = body.mode === 'bypass' ? MODE_BYPASS_SUFFIX : MODE_ASK_SUFFIX;
    const upstream = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'x-api-key': requiredEnv('ANTHROPIC_API_KEY'),
        'anthropic-version': '2023-06-01',
        'content-type': 'application/json',
        accept: 'text/event-stream',
      },
      body: JSON.stringify({
        model: process.env.PEER_AGGREGATOR_MODEL || 'claude-sonnet-4-6',
        max_tokens: 4096,
        stream: true,
        system: [{ type: 'text', text: `${AGGREGATOR_SYSTEM}\n\n${modeSuffix}` }],
        messages: [{ role: 'user', content: [{ type: 'text', text: userMessage }] }],
      }),
    });

    if (!upstream.ok) {
      const text = await upstream.text();
      res.statusCode = upstream.status;
      return res.end(text);
    }

    res.statusCode = 200;
    res.setHeader('content-type', 'text/event-stream; charset=utf-8');
    res.setHeader('cache-control', 'no-cache, no-transform');
    for await (const chunk of upstream.body) {
      res.write(chunk);
    }
    await recordUsage(auth, 'recording');
    res.end();
  } catch (err) {
    handleError(res, err);
  }
}

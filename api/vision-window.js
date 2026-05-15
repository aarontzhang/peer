import {
  WINDOW_SYSTEM,
  assertRecordingQuota,
  extractJson,
  handleError,
  readJson,
  requireUser,
  requiredEnv,
  sendJson,
} from './_backend.js';

export const config = {
  api: { bodyParser: { sizeLimit: '16mb' } },
};

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });
    const auth = await requireUser(req);
    await assertRecordingQuota(auth);

    const body = await readJson(req);
    const frames = Array.isArray(body.frames) ? body.frames : [];
    const content = frames.map((frame) => ({
      type: 'image',
      source: {
        type: 'base64',
        media_type: frame.mediaType || 'image/jpeg',
        data: frame.data,
      },
    }));
    content.push({
      type: 'text',
      text: `Window ${(body.index ?? 0) + 1} covers about ${Number(body.tStart || 0).toFixed(1)}s..about ${Number(body.tEnd || 0).toFixed(1)}s of the recording.\n\nNarration in this window:\n${body.transcriptSlice || '(no narration in this window)'}\n\nReturn the JSON object now.`,
    });

    const upstream = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'x-api-key': requiredEnv('ANTHROPIC_API_KEY'),
        'anthropic-version': '2023-06-01',
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        model: process.env.PEER_WINDOW_MODEL || 'claude-sonnet-4-6',
        max_tokens: 1024,
        system: [{ type: 'text', text: WINDOW_SYSTEM }],
        messages: [{ role: 'user', content }],
      }),
    });

    const payload = await upstream.json();
    if (!upstream.ok) return sendJson(res, upstream.status, payload);
    const text = payload.content?.find((item) => item.text)?.text || '';
    sendJson(res, 200, extractJson(text));
  } catch (err) {
    handleError(res, err);
  }
}

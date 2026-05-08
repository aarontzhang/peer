import { handleError, readJson, requireDeviceAuth, requiredEnv, sendJson } from './_backend.js';

export const config = {
  api: { bodyParser: { sizeLimit: '16mb' } },
};

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });
    await requireDeviceAuth(req);

    const body = await readJson(req);
    const audioBase64 = String(body.audioBase64 || '');
    if (!audioBase64) return sendJson(res, 400, { error: 'audioBase64 is required' });

    const audio = Buffer.from(audioBase64, 'base64');
    const form = new FormData();
    form.append(
      'file',
      new Blob([audio], { type: body.mimeType || 'audio/mpeg' }),
      'audio.mp3',
    );
    form.append('model', 'whisper-1');
    form.append('response_format', 'verbose_json');
    form.append('temperature', '0');

    const upstream = await fetch('https://api.openai.com/v1/audio/transcriptions', {
      method: 'POST',
      headers: { authorization: `Bearer ${requiredEnv('OPENAI_API_KEY')}` },
      body: form,
    });

    const text = await upstream.text();
    if (!upstream.ok) {
      res.statusCode = upstream.status;
      res.setHeader('content-type', 'application/json; charset=utf-8');
      return res.end(text);
    }

    res.statusCode = 200;
    res.setHeader('content-type', 'application/json; charset=utf-8');
    res.end(text);
  } catch (err) {
    handleError(res, err);
  }
}

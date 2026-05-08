import crypto from 'node:crypto';
import { handleError, readJson, sendJson } from './_backend.js';

export default async function handler(req, res) {
  try {
    if (req.method === 'GET') return renderLogin(req, res);
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });

    const body = await readJson(req);
    const email = String(body.email || '').trim().toLowerCase();
    const deviceId = String(body.deviceId || '').trim();
    const inviteCode = String(body.inviteCode || '').trim();

    if (!email || !email.includes('@')) return sendJson(res, 400, { error: 'valid email is required' });
    if (!deviceId) return sendJson(res, 400, { error: 'deviceId is required' });
    if (process.env.PEER_BETA_INVITE_CODE && inviteCode !== process.env.PEER_BETA_INVITE_CODE) {
      return sendJson(res, 403, { error: 'invalid beta invite code' });
    }

    const token = `peer_${crypto.randomBytes(32).toString('base64url')}`;
    await upsertDeviceToken({ email, deviceId, token });
    sendJson(res, 200, {
      token,
      deepLink: `peer://auth?token=${encodeURIComponent(token)}`,
    });
  } catch (err) {
    handleError(res, err);
  }
}

function renderLogin(req, res) {
  const url = new URL(req.url, `https://${req.headers.host}`);
  const deviceId = url.searchParams.get('device_id') || crypto.randomUUID();
  res.statusCode = 200;
  res.setHeader('content-type', 'text/html; charset=utf-8');
  res.end(`<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Peer Login</title>
  <style>
    body{margin:0;font:14px/1.5 -apple-system,BlinkMacSystemFont,"SF Pro Text",Segoe UI,sans-serif;background:#151515;color:#f5f2ec;display:grid;place-items:center;min-height:100vh}
    main{width:min(420px,calc(100vw - 40px));border:1px solid #3a3833;border-radius:12px;padding:24px;background:#20201e}
    h1{font-size:22px;margin:0 0 8px}
    p{color:#b9b3a8;margin:0 0 18px}
    label{display:block;font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:#a7a198;margin:14px 0 6px}
    input,button{width:100%;box-sizing:border-box;border-radius:8px;border:1px solid #48443d;padding:10px 12px;font:inherit}
    input{background:#121211;color:#f5f2ec}
    button{margin-top:16px;background:#3c79c8;color:white;border-color:#5590da;font-weight:600;cursor:pointer}
    code{display:block;white-space:pre-wrap;word-break:break-all;background:#111;padding:12px;border-radius:8px;margin-top:14px}
  </style>
</head>
<body>
  <main>
    <h1>Sign in to Peer</h1>
    <p>Enter your beta account email. Peer will create a device token for this Mac.</p>
    <form id="form">
      <input type="hidden" name="deviceId" value="${escapeHtml(deviceId)}">
      <label>Email</label>
      <input name="email" type="email" autocomplete="email" required autofocus>
      <label>Beta invite code</label>
      <input name="inviteCode" type="password" autocomplete="one-time-code">
      <button>Continue</button>
    </form>
    <div id="result"></div>
  </main>
  <script>
    document.getElementById('form').addEventListener('submit', async (event) => {
      event.preventDefault();
      const data = Object.fromEntries(new FormData(event.currentTarget));
      const res = await fetch('/api/desktop-login', { method:'POST', headers:{'content-type':'application/json'}, body: JSON.stringify(data) });
      const json = await res.json();
      const target = document.getElementById('result');
      if (!res.ok) { target.innerHTML = '<code>' + json.error + '</code>'; return; }
      target.innerHTML = '<p>Paste this token into Peer Settings:</p><code>' + json.token + '</code><p><a style="color:#8dbdff" href="' + json.deepLink + '">Open Peer</a></p>';
    });
  </script>
</body>
</html>`);
}

async function upsertDeviceToken({ email, deviceId, token }) {
  const supabaseUrl = process.env.SUPABASE_URL;
  const serviceKey = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!supabaseUrl || !serviceKey) {
    if (process.env.PEER_BACKEND_BYPASS_AUTH === '1') return;
    throw new Error('missing Supabase environment');
  }

  const response = await fetch(`${supabaseUrl.replace(/\/$/, '')}/rest/v1/device_tokens`, {
    method: 'POST',
    headers: {
      apikey: serviceKey,
      authorization: `Bearer ${serviceKey}`,
      'content-type': 'application/json',
      prefer: 'resolution=merge-duplicates,return=minimal',
    },
    body: JSON.stringify({
      user_id: email,
      device_id: deviceId,
      token,
      expires_at: new Date(Date.now() + 1000 * 60 * 60 * 24 * 90).toISOString(),
    }),
  });
  if (!response.ok) throw new Error(`Supabase token write failed: ${response.status}`);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[char]);
}

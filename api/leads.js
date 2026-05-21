import { createHash } from 'node:crypto';

import { handleError, readJson, requiredEnv, sendJson } from './_backend.js';

export const config = {
  api: { bodyParser: { sizeLimit: '32kb' } },
};

const ALLOWED_SOURCES = new Set(['contact_sales', 'download_gate']);
const EMAIL_RATE_WINDOW_SECONDS = 60;
const EMAIL_RATE_MAX_PER_WINDOW = 2;
const IP_RATE_WINDOW_SECONDS = 60;
const IP_RATE_MAX_PER_WINDOW = 8;
const MAX_NAME = 120;
const MAX_COMPANY = 120;
const MAX_USE_CASE = 4000;
const MAX_USER_AGENT = 400;

// Loose but standards-aligned: anchor, single @, no whitespace, dot in domain.
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });

    const body = await readJson(req);

    // Honeypot field. Bots fill every input; humans never see this one.
    if (typeof body.website === 'string' && body.website.trim() !== '') {
      return sendJson(res, 200, { ok: true });
    }

    const source = String(body.source || '').trim();
    if (!ALLOWED_SOURCES.has(source)) {
      return sendJson(res, 400, { error: 'invalid source' });
    }

    const email = normalizeEmail(body.email);
    if (!email) return sendJson(res, 400, { error: 'a valid email is required' });

    const name = clampString(body.name, MAX_NAME);
    const company = clampString(body.company, MAX_COMPANY);
    const useCase = clampString(body.useCase, MAX_USE_CASE);
    const userAgent = clampString(req.headers['user-agent'] || '', MAX_USER_AGENT) || null;
    const ipHash = hashIp(clientIp(req));

    const supabaseUrl = requiredEnv('SUPABASE_URL').replace(/\/$/, '');
    const serviceKey = requiredEnv('SUPABASE_SERVICE_ROLE_KEY');

    const rateOk = await checkRateLimit({ supabaseUrl, serviceKey, email, ipHash });
    if (!rateOk) {
      return sendJson(res, 429, { error: 'too many submissions — try again in a minute' });
    }

    const insertResponse = await fetch(`${supabaseUrl}/rest/v1/contact_sales_leads`, {
      method: 'POST',
      headers: {
        apikey: serviceKey,
        authorization: `Bearer ${serviceKey}`,
        'content-type': 'application/json',
        prefer: 'return=minimal',
      },
      body: JSON.stringify({
        source,
        email,
        name: name || null,
        company: company || null,
        use_case: useCase || null,
        ip_hash: ipHash,
        user_agent: userAgent,
      }),
    });

    if (!insertResponse.ok) {
      const text = await insertResponse.text().catch(() => '');
      console.error('contact_sales_leads insert failed', insertResponse.status, text);
      return sendJson(res, 502, { error: 'could not record submission' });
    }

    // Slack is best-effort. Lead is already persisted; a missing or broken
    // webhook should not break the user's submit flow.
    notifySlack({ source, email, name, company, useCase }).catch((err) => {
      console.error('Slack notification failed', err);
    });

    if (source === 'download_gate') {
      const downloadUrl = process.env.PEER_MACOS_DOWNLOAD_URL
        || 'https://github.com/aarontzhang/peer/releases/latest/download/Peer.dmg';
      return sendJson(res, 200, { ok: true, downloadUrl });
    }

    sendJson(res, 200, { ok: true });
  } catch (err) {
    handleError(res, err);
  }
}

function normalizeEmail(input) {
  if (typeof input !== 'string') return null;
  const trimmed = input.trim().toLowerCase();
  if (trimmed.length === 0 || trimmed.length > 320) return null;
  if (!EMAIL_RE.test(trimmed)) return null;
  return trimmed;
}

function clampString(input, max) {
  if (typeof input !== 'string') return '';
  const trimmed = input.trim();
  if (trimmed.length === 0) return '';
  return trimmed.length > max ? trimmed.slice(0, max) : trimmed;
}

function clientIp(req) {
  const fwd = req.headers['x-forwarded-for'];
  if (typeof fwd === 'string' && fwd.length > 0) {
    return fwd.split(',')[0].trim();
  }
  const real = req.headers['x-real-ip'];
  if (typeof real === 'string' && real.length > 0) return real.trim();
  return req.socket?.remoteAddress || '';
}

function hashIp(ip) {
  if (!ip) return null;
  // Salt with a project-specific string so the hash isn't a generic
  // SHA-256(IP) lookup. We don't need a secret, just a separator.
  return createHash('sha256').update(`peer-leads:${ip}`).digest('hex').slice(0, 32);
}

async function checkRateLimit({ supabaseUrl, serviceKey, email, ipHash }) {
  const emailSince = new Date(Date.now() - EMAIL_RATE_WINDOW_SECONDS * 1000).toISOString();
  const ipSince = new Date(Date.now() - IP_RATE_WINDOW_SECONDS * 1000).toISOString();

  const checks = [
    countRecent({
      supabaseUrl,
      serviceKey,
      column: 'email',
      value: email,
      since: emailSince,
      limit: EMAIL_RATE_MAX_PER_WINDOW,
    }),
  ];
  if (ipHash) {
    checks.push(
      countRecent({
        supabaseUrl,
        serviceKey,
        column: 'ip_hash',
        value: ipHash,
        since: ipSince,
        limit: IP_RATE_MAX_PER_WINDOW,
      }),
    );
  }
  const results = await Promise.all(checks);
  return results.every((ok) => ok);
}

async function countRecent({ supabaseUrl, serviceKey, column, value, since, limit }) {
  const params = new URLSearchParams({
    select: 'id',
    [column]: `eq.${value}`,
    created_at: `gte.${since}`,
    limit: String(limit + 1),
  });
  const response = await fetch(`${supabaseUrl}/rest/v1/contact_sales_leads?${params}`, {
    headers: {
      apikey: serviceKey,
      authorization: `Bearer ${serviceKey}`,
      prefer: 'count=exact',
    },
  });
  if (!response.ok) {
    // If the rate-limit check itself fails, fall back to allowing — we'd
    // rather risk an extra submission than block real leads behind a
    // transient Supabase blip.
    console.error('rate limit check failed', column, response.status);
    return true;
  }
  const contentRange = response.headers.get('content-range') || '';
  const total = Number.parseInt(contentRange.split('/').pop() || '0', 10);
  return Number.isFinite(total) ? total < limit : true;
}

async function notifySlack({ source, email, name, company, useCase }) {
  const webhook = process.env.PEER_SALES_SLACK_WEBHOOK;
  if (!webhook) return;
  const heading = source === 'contact_sales' ? 'New contact-sales lead' : 'New download request';
  const fields = [
    field('Email', email),
    field('Name', name),
    field('Company', company),
    field('Use case', useCase ? truncate(useCase, 1000) : ''),
  ].filter(Boolean);

  const text = [`*${escapeSlack(heading)}*`, ...fields].join('\n');

  await fetch(webhook, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ text }),
  });
}

function field(label, value) {
  if (!value) return null;
  return `• ${escapeSlack(label)}: ${escapeSlack(value)}`;
}

function truncate(s, max) {
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

// Slack's mrkdwn treats &, <, > as control characters. Escape so a pasted
// "<script>" or "alice@bar & baz" doesn't break the message or open holes.
function escapeSlack(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

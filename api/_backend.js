import { createRemoteJWKSet, jwtVerify } from 'jose';

export const WINDOW_SYSTEM = `You take one window of a screen recording (a few keyframes plus the user's narration over that window) and return a compact JSON note. A downstream model will combine the per-window notes into a single, refined instruction prompt for a coding agent.

You will receive:
- A small ordered set of JPEG keyframes from one segment of the recording.
- The aligned narration transcript for that segment.
- The segment's time range relative to the full clip.

THE CURSOR IS THE PRIMARY SIGNAL. The user is narrating while pointing - the mouse cursor is how they indicate what "this", "here", "that line", "this button" refer to. In every frame, FIRST locate the cursor, THEN read what it is on, near, or hovering over. Whatever the cursor is touching is almost certainly the subject of the user's speech in that moment. Do not produce a generic frame description; produce a cursor-anchored one.

Return ONLY this JSON object, nothing else:
{
  "pointing": "REQUIRED unless the cursor is genuinely unrelated to the user's speech. Name the exact thing the cursor is on, hovering over, or has just clicked/selected. Quote exact text under the cursor when present. If the cursor moves between distinct targets within the window, list them in order separated by ' -> '.",
  "userSpeech": "what the user said in this window - keep their phrasing; trim only filler, false starts, and pure repetition",
  "visibleContext": ["concrete things visible in the frames that the user is referring to or working with - but only items the cursor or the user's speech actually invokes. Prefer items the cursor touched. File paths, function/variable names, exact button/menu/tab labels, error text, URLs, short code snippets."]
}

Rules:
- Cursor first. Read the frames in cursor -> speech -> surroundings order, not left-to-right. The cursor's target is the heart of every note.
- Only describe things you can actually see in the frames or hear in the narration. Never invent.
- Prefer exact strings over paraphrase. If a button says "Save Draft", write "Save Draft". If the cursor is on code, quote the exact token/line.
- If the cursor is hard to find in a frame, say so in "pointing" rather than guessing.
- Keep arrays tight, no more than 6 items. Empty arrays are fine; "pointing" should almost never be empty when the cursor is visible.`;

export const AGGREGATOR_SYSTEM = `You take per-window notes from a screen recording - the user narrating over their own screen - and produce a single, refined prompt that the user can hand to a coding agent.

You will receive:
- The full transcript with timestamps.
- An ordered list of per-window notes with the user's speech, visible on-screen context, and what they were pointing at.
- Optional clip metadata.

Your job is to restate the user's request more clearly and completely than they said it out loud. The agent who reads your output will not see the video, will not hear the narration, and will not have the cursor as a pointer - so every "this", "that", "here", "the thing I'm looking at" must be resolved into named, concrete references.

How to write it:
- Write in the user's voice, first person.
- Resolve every deictic reference. "This button" -> the exact label and where it lives. "That file" -> the file path. "Here" -> the function/line/screen.
- Weave the on-screen context inline. Name files, functions, exact UI labels, error text, URLs, and short code snippets the user pointed at.
- Preserve every actionable detail and constraint the user mentioned. Cut filler, false starts, and pure repetition; reorder for clarity if they jumped around.
- Do not invent steps, rationale, acceptance criteria, or open questions the user did not raise.

Output format:
- Plain text only. No markdown formatting of any kind.
- No top-level title or summary line - just the prompt itself.
- Short paragraphs in plain prose. Err on the side of being specific and self-contained over being terse, but never pad with content the recording did not supply.`;

export async function readJson(req) {
  if (req.body && typeof req.body === 'object') return req.body;
  if (typeof req.body === 'string') return JSON.parse(req.body);

  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const raw = Buffer.concat(chunks).toString('utf8');
  return raw ? JSON.parse(raw) : {};
}

export function sendJson(res, status, body) {
  res.statusCode = status;
  res.setHeader('content-type', 'application/json; charset=utf-8');
  res.end(JSON.stringify(body));
}

export function bearerToken(req) {
  const header = req.headers.authorization || '';
  const match = /^Bearer\s+(.+)$/i.exec(header);
  return match?.[1]?.trim() || null;
}

let cachedSecret = null;
function jwtSecret() {
  if (!cachedSecret) {
    cachedSecret = new TextEncoder().encode(requiredEnv('SUPABASE_JWT_SECRET'));
  }
  return cachedSecret;
}

let cachedJwks = null;
function supabaseJwks() {
  if (!cachedJwks) {
    const supabaseUrl = requiredEnv('SUPABASE_URL').replace(/\/$/, '');
    cachedJwks = createRemoteJWKSet(new URL(`${supabaseUrl}/auth/v1/.well-known/jwks.json`));
  }
  return cachedJwks;
}

async function verifySupabaseJwt(token) {
  try {
    return await jwtVerify(token, supabaseJwks(), {
      audience: 'authenticated',
      algorithms: ['ES256', 'RS256', 'EdDSA'],
    });
  } catch (jwksError) {
    try {
      return await jwtVerify(token, jwtSecret(), {
        audience: 'authenticated',
        algorithms: ['HS256'],
      });
    } catch {
      throw jwksError;
    }
  }
}

export async function requireUser(req) {
  const token = bearerToken(req);
  if (!token) throw httpError(401, 'missing bearer token');

  if (process.env.PEER_BACKEND_BYPASS_AUTH === '1') {
    return { userId: 'dev', email: 'dev@local', token };
  }

  try {
    const { payload } = await verifySupabaseJwt(token);
    return { userId: payload.sub, email: payload.email ?? null, token };
  } catch {
    throw httpError(401, 'invalid or expired token');
  }
}

export async function recordUsage(auth, kind) {
  if (process.env.PEER_BACKEND_BYPASS_AUTH === '1') return;
  const supabaseUrl = process.env.SUPABASE_URL;
  const serviceKey = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!supabaseUrl || !serviceKey) return;
  await fetch(`${supabaseUrl.replace(/\/$/, '')}/rest/v1/recording_usage`, {
    method: 'POST',
    headers: {
      apikey: serviceKey,
      authorization: `Bearer ${serviceKey}`,
      'content-type': 'application/json',
      prefer: 'return=minimal',
    },
    body: JSON.stringify({ user_id: auth.userId, kind }),
  }).catch(() => {});
}

export function requiredEnv(name) {
  const value = process.env[name];
  if (!value) throw httpError(500, `missing ${name}`);
  return value;
}

export function httpError(status, message) {
  const err = new Error(message);
  err.status = status;
  return err;
}

export function handleError(res, err) {
  const status = err.status || 500;
  sendJson(res, status, { error: err.message || 'internal server error' });
}

export function extractJson(text) {
  const trimmed = text.trim();
  try {
    return JSON.parse(trimmed);
  } catch {}
  const start = trimmed.indexOf('{');
  const end = trimmed.lastIndexOf('}');
  if (start >= 0 && end > start) {
    try {
      return JSON.parse(trimmed.slice(start, end + 1));
    } catch {}
  }
  return { userSpeech: trimmed, visibleContext: [], pointing: '' };
}

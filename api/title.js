import {
  handleError,
  readJson,
  requireUser,
  requiredEnv,
  sendJson,
} from './_backend.js';

const TITLE_SYSTEM = `You write a 3-5 word title that captures the essence of a coding task a user has described.

Style:
- Imperative, present tense ("Find collaborator settings", "Fix login redirect", "Add dark-mode toggle").
- 3 to 5 words. Never more than 6.
- No quotes, no trailing punctuation, no markdown.
- Plain ASCII apostrophes only.
- Title case is fine but lowercase is also fine — match how a developer would write a commit subject.

Output the title and nothing else — no preamble, no explanation, no bullets.`;

export const config = {
  api: { bodyParser: { sizeLimit: '512kb' } },
};

export default async function handler(req, res) {
  try {
    if (req.method !== 'POST') return sendJson(res, 405, { error: 'method not allowed' });
    await requireUser(req);

    const body = await readJson(req);
    const prompt = String(body.prompt || '').trim();
    if (!prompt) return sendJson(res, 400, { error: 'prompt is required' });

    // Cap input — titles only need the gist. Sending the whole prompt wastes
    // tokens and risks the model fixating on a late detail.
    const truncated = prompt.length > 4000 ? prompt.slice(0, 4000) : prompt;

    const upstream = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'x-api-key': requiredEnv('ANTHROPIC_API_KEY'),
        'anthropic-version': '2023-06-01',
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        model: process.env.PEER_TITLE_MODEL || 'claude-haiku-4-5',
        max_tokens: 32,
        system: [{ type: 'text', text: TITLE_SYSTEM }],
        messages: [
          {
            role: 'user',
            content: [
              {
                type: 'text',
                text: `Coding task description:\n\n${truncated}\n\nReturn the title now.`,
              },
            ],
          },
        ],
      }),
    });

    const payload = await upstream.json();
    if (!upstream.ok) return sendJson(res, upstream.status, payload);

    const raw = payload.content?.find((item) => item.text)?.text || '';
    const title = sanitizeTitle(raw);
    sendJson(res, 200, { title });
  } catch (err) {
    handleError(res, err);
  }
}

function sanitizeTitle(input) {
  let t = String(input || '').trim();
  // Drop any leading/trailing surrounding quotes the model may add.
  t = t.replace(/^["'`“”‘’]+|["'`“”‘’]+$/g, '');
  // Take only the first non-empty line.
  t = t.split(/\r?\n/).map((s) => s.trim()).find(Boolean) || '';
  // Strip trailing terminal punctuation.
  t = t.replace(/[.!?…]+$/g, '');
  // Hard cap at 60 chars so a misbehaving model can't break the row layout.
  if (t.length > 60) t = t.slice(0, 60).trimEnd();
  return t;
}

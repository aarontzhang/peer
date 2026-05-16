import { describe, expect, it } from 'vitest';
import { firstPlainTextLine, toPlainText } from './plainText';

describe('plain text helpers', () => {
  it('strips common markdown while preserving useful text', () => {
    expect(toPlainText(`
# Main task

> quoted context

- [x] **Fix** the [button](https://example.com)
- Capture \`state\`

\`\`\`ts
const hidden = true;
\`\`\`
`)).toBe('Main task\n\nquoted context\n\nFix the button\nCapture state\n\nconst hidden = true;');
  });

  it('returns the first non-empty plain text line', () => {
    expect(firstPlainTextLine('\n\n## Title\nMore')).toBe('Title');
  });
});

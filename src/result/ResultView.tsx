import { useEffect, useMemo, useRef, useState } from 'react';
import { type Recording } from '@/lib/ipc';
import { toPlainText } from '@/lib/plainText';

type Props = {
  recording: Recording | null;
  liveBody: string | null;
  liveThinking: string | null;
  isStreaming: boolean;
  onCopyPrompt: (text: string) => Promise<void>;
};

export function ResultView({ recording, liveBody, liveThinking, isStreaming, onCopyPrompt }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const body = useMemo(() => toPlainText(liveBody ?? recording?.body ?? ''), [liveBody, recording?.body]);

  // Prefer the live thinking event so it's visible the instant per-window
  // analyses finish, well before the prompt finishes streaming.
  const thinking = liveThinking ?? recording?.thinking ?? null;

  // Typewriter buffer: the source `body` arrives in chunky deltas from the
  // model. We drain those chunks character-by-character on rAF so the user
  // sees a smooth ChatGPT-style stream instead of step jumps.
  const recordingId = recording?.id ?? null;
  const [displayed, setDisplayed] = useState(body);

  // Snap to the current body whenever the user switches recordings — we
  // never want to typewrite an already-finished history entry.
  useEffect(() => {
    setDisplayed(body);
    // intentionally only on recordingId change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recordingId]);

  useEffect(() => {
    if (displayed === body) return;
    // If the new body diverges from what we've already shown (e.g. a
    // recording was re-run), snap rather than animate from the wrong prefix.
    if (!body.startsWith(displayed)) {
      setDisplayed(body);
      return;
    }
    let raf = 0;
    const startTime = performance.now();
    const startLen = displayed.length;
    const totalLen = body.length;
    const tick = () => {
      const elapsed = performance.now() - startTime;
      const backlog = totalLen - startLen;
      // Adaptive rate: ~100 chars/sec at small backlogs, accelerating as the
      // queue grows so we never fall further behind on big chunks.
      const charsPerMs = Math.min(1.5, 0.1 + backlog / 140);
      const advance = Math.max(1, Math.round(elapsed * charsPerMs));
      const nextLen = Math.min(totalLen, startLen + advance);
      setDisplayed(body.slice(0, nextLen));
      if (nextLen < totalLen) {
        raf = requestAnimationFrame(tick);
      }
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [body, displayed]);

  // Stick-to-bottom: only auto-scroll while the user is already pinned near
  // the end. The moment they scroll up to read, we stop yanking them back.
  // (The programmatic scroll below always lands at the bottom, so it
  // naturally keeps the flag true without needing to be filtered out.)
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      stickToBottomRef.current = distanceFromBottom < 24;
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, []);

  // Reset to "stuck" each time we switch into a streaming recording.
  useEffect(() => {
    if (isStreaming) stickToBottomRef.current = true;
  }, [isStreaming, recordingId]);

  useEffect(() => {
    if (!isStreaming) return;
    if (!stickToBottomRef.current) return;
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [displayed, isStreaming]);

  if (!recording) {
    return null;
  }

  if (recording.status === 'failed') {
    return (
      <div className="main">
        <div className="main__bar" data-tauri-drag-region />
        <div className="main__scroll">
          <div className="md">
            <h1>Recording failed</h1>
            <p style={{ color: 'var(--color-fg-muted)' }}>
              {recording.error ?? 'Unknown error.'}
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (recording.status === 'canceled') {
    return (
      <div className="main">
        <div className="main__bar" data-tauri-drag-region />
        <div className="main__scroll">
          <div className="md">
            <h1 style={{ color: 'var(--color-fg-muted)' }}>Cancelled</h1>
            <p style={{ color: 'var(--color-fg-dim)' }}>
              You discarded this capture from the pill. The video has been deleted, but the entry
              stays here so you have a record of when it happened.
            </p>
          </div>
        </div>
      </div>
    );
  }

  const inEarlyState =
    !body &&
    (recording.status === 'recording' ||
      recording.status === 'processing' ||
      recording.status === 'stopped');

  if (inEarlyState && !thinking) {
    const heading =
      recording.status === 'recording' ? 'Recording…'
      : recording.status === 'stopped' ? 'Captured.'
      : 'Analyzing…';
    const sub =
      recording.status === 'stopped'
        ? 'Press Enter to analyze, Delete to discard, or use the pill buttons.'
        : "The instruction set will stream in here as soon as it's ready.";
    return (
      <div className="main">
        <div className="main__bar" data-tauri-drag-region />
        <div className="main__scroll">
          <div className="md">
            <h1 style={{ color: 'var(--color-fg-muted)' }}>{heading}</h1>
            <p style={{ color: 'var(--color-fg-dim)' }}>{sub}</p>
          </div>
        </div>
      </div>
    );
  }

  const onCopy = async () => {
    if (!body) return;
    await onCopyPrompt(body);
  };

  // Auto-expand the thinking pane while we're still waiting on the prompt
  // (or actively streaming it). Once the prompt is done it collapses again so
  // the refined output stays the focus.
  const thinkingOpen = isStreaming || !body;
  const showCursor = isStreaming || displayed.length < body.length;

  return (
    <div className="main">
      <div className="main__bar" data-tauri-drag-region>
        <div className="main__actions">
          <button
            className="icon-btn"
            onClick={onCopy}
            disabled={!body}
            aria-label="Copy"
            title="Copy"
          >
            <CopyIcon />
          </button>
        </div>
      </div>
      <div className="main__scroll" ref={scrollRef}>
        {thinking && (
          <details className="thinking thinking--top" open={thinkingOpen}>
            <summary className="thinking__summary">
              <ChevronIcon />
              <span>{thinkingOpen ? 'Thinking' : 'Show thinking'}</span>
            </summary>
            <div className="thinking__body">{thinking}</div>
          </details>
        )}
        {body ? (
          <div className="prompt-body">
            {displayed}
            {showCursor && <span className="streaming-cursor" />}
          </div>
        ) : (
          <div className="md prompt-pending">
            <p style={{ color: 'var(--color-fg-dim)' }}>
              Writing the refined prompt…<span className="streaming-cursor" />
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

function ChevronIcon() {
  return (
    <svg
      className="thinking__chev"
      viewBox="0 0 16 16"
      width="11"
      height="11"
      aria-hidden
    >
      <path
        d="M5 4l5 4-5 4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <rect x="3" y="2.5" width="8" height="9.5" rx="1.6" ry="1.6"
            fill="none" stroke="currentColor" strokeWidth="1.3" />
      <rect x="5.5" y="5" width="8" height="9.5" rx="1.6" ry="1.6"
            fill="none" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

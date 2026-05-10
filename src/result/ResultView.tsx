import { useEffect, useMemo, useRef, useState } from 'react';
import { type Recording } from '@/lib/ipc';
import { toPlainText } from '@/lib/plainText';

type Props = {
  recording: Recording | null;
  liveBody: string | null;
  liveThinking: string | null;
  isStreaming: boolean;
  onCopyPrompt: (text: string) => Promise<void>;
  onRequestDelete: () => void;
  onRetry: () => void;
  retryDisabled?: boolean;
};

export function ResultView({ recording, liveBody, liveThinking, isStreaming, onCopyPrompt, onRequestDelete, onRetry, retryDisabled }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const body = useMemo(() => toPlainText(liveBody ?? recording?.body ?? ''), [liveBody, recording?.body]);

  // Prefer the live thinking event so it's visible the instant per-window
  // analyses finish, well before the prompt finishes streaming.
  const rawThinking = liveThinking ?? recording?.thinking ?? null;
  const thinking = useMemo(
    () => (rawThinking ? toPlainText(rawThinking) : null),
    [rawThinking],
  );

  // User-controlled override of the thinking pane's open state. Null = follow
  // the auto rule (open while streaming, collapsed once the prompt lands);
  // boolean = the user clicked the toggle and wants their choice respected.
  const [thinkingOverride, setThinkingOverride] = useState<boolean | null>(null);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | null>(null);

  // Typewriter buffer: the source `body` arrives in chunky deltas from the
  // model. We drain those chunks character-by-character on rAF so the user
  // sees a smooth ChatGPT-style stream instead of step jumps.
  const recordingId = recording?.id ?? null;
  const [displayed, setDisplayed] = useState(body);

  // Snap to the current body whenever the user switches recordings — we
  // never want to typewrite an already-finished history entry.
  useEffect(() => {
    setDisplayed(body);
    setThinkingOverride(null);
    setCopied(false);
    if (copiedTimer.current) {
      window.clearTimeout(copiedTimer.current);
      copiedTimer.current = null;
    }
    // intentionally only on recordingId change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recordingId]);

  useEffect(() => {
    return () => {
      if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    };
  }, []);

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
  // the end. We listen for *user-driven* input (wheel, touch, keys) rather
  // than the generic `scroll` event so our own programmatic scroll-to-bottom
  // can't race the handler and clobber the user's intent.
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const recheck = () => {
      // Read after the browser has applied the user's scroll delta.
      requestAnimationFrame(() => {
        if (!el.isConnected) return;
        const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
        stickToBottomRef.current = distanceFromBottom < 24;
      });
    };
    // Break stick *synchronously* on any user-driven scroll input. If we
    // wait for the rAF recheck, a streaming delta can land in between and
    // re-snap the viewport to the bottom before we update the flag — which
    // is what made it feel impossible to read the thinking pane while the
    // prompt was still typing. The rAF recheck still runs and re-enables
    // stick when the user has scrolled themselves back to the bottom.
    const breakAndRecheck = () => {
      stickToBottomRef.current = false;
      recheck();
    };
    const onKey = (e: KeyboardEvent) => {
      if (
        e.key === 'ArrowUp' || e.key === 'ArrowDown' ||
        e.key === 'PageUp' || e.key === 'PageDown' ||
        e.key === 'Home' || e.key === 'End' ||
        e.key === ' '
      ) breakAndRecheck();
    };
    el.addEventListener('wheel', breakAndRecheck, { passive: true });
    el.addEventListener('touchmove', breakAndRecheck, { passive: true });
    window.addEventListener('keydown', onKey);
    return () => {
      el.removeEventListener('wheel', breakAndRecheck);
      el.removeEventListener('touchmove', breakAndRecheck);
      window.removeEventListener('keydown', onKey);
    };
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
        <div className="main__bar" data-tauri-drag-region>
          <div className="main__actions">
            <button
              className="icon-btn"
              onClick={onRetry}
              disabled={retryDisabled}
              aria-label="Analyze the video"
              title="Analyze the video"
            >
              <RetryIcon />
            </button>
            <button
              className="icon-btn icon-btn--danger"
              onClick={onRequestDelete}
              aria-label="Delete recording"
              title="Delete recording"
            >
              <TrashIcon />
            </button>
          </div>
        </div>
        <div className="main__scroll">
          <div className="md">
            <h1 style={{ color: 'var(--color-fg-muted)' }}>Cancelled</h1>
            <p style={{ color: 'var(--color-fg-dim)' }}>
              You cancelled the recording before it was analyzed. The video is still here —
              you can analyze it now, or delete it for good.
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
    setCopied(true);
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => {
      setCopied(false);
      copiedTimer.current = null;
    }, 1600);
  };

  // Auto-expand the thinking pane while we're still waiting on the prompt
  // (or actively streaming it). Once the prompt is done it collapses again so
  // the refined output stays the focus — unless the user has clicked the
  // toggle, in which case their choice wins.
  const autoOpen = isStreaming || !body;
  const thinkingOpen = thinkingOverride ?? autoOpen;
  const onToggleThinking = () => setThinkingOverride(!thinkingOpen);

  return (
    <div className="main">
      <div className="main__bar" data-tauri-drag-region>
        {thinking && (
          <div className="main__leading">
            <button
              type="button"
              className={`thinking-toggle${thinkingOpen ? ' thinking-toggle--open' : ''}`}
              onClick={onToggleThinking}
              aria-expanded={thinkingOpen}
              data-no-drag
            >
              <ChevronIcon />
              <span>{thinkingOpen ? 'Hide thinking' : 'Show thinking'}</span>
            </button>
          </div>
        )}
        <div className="main__actions">
          <button
            className={`icon-btn${copied ? ' icon-btn--solid' : ''}`}
            onClick={onCopy}
            disabled={!body}
            aria-label={copied ? 'Copied' : 'Copy'}
            title={copied ? 'Copied' : 'Copy'}
          >
            {copied ? <CheckIcon /> : <CopyIcon />}
          </button>
          <button
            className="icon-btn icon-btn--danger"
            onClick={onRequestDelete}
            aria-label="Delete recording"
            title="Delete recording"
          >
            <TrashIcon />
          </button>
        </div>
      </div>
      <div className="main__scroll" ref={scrollRef}>
        {thinking && thinkingOpen && (
          <div className="thinking__body">{thinking}</div>
        )}
        {thinking && thinkingOpen && body && <hr className="thinking-sep" />}
        {body ? (
          <div className="prompt-body">{displayed}</div>
        ) : (
          <div className="md prompt-pending">
            <p style={{ color: 'var(--color-fg-dim)' }}>Writing the refined prompt…</p>
          </div>
        )}
      </div>
    </div>
  );
}

function CheckIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path
        d="M3.5 8.4l3 3 6-6"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
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

function RetryIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path
        d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <path
        d="M13.5 2.5v3h-3"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path
        d="M3 4.5h10M6.5 4.5V3.2c0-.4.3-.7.7-.7h1.6c.4 0 .7.3.7.7v1.3M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8M7 7v4M9 7v4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

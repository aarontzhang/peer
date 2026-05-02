import { useEffect, useMemo, useRef, useState } from 'react';
import { marked } from 'marked';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { type Recording } from '@/lib/ipc';

type Props = {
  recording: Recording | null;
  liveBody: string | null;
  isStreaming: boolean;
};

export function ResultView({ recording, liveBody, isStreaming }: Props) {
  const [copied, setCopied] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  const body = liveBody ?? recording?.body ?? '';
  const html = useMemo(() => {
    if (!body) return '';
    return marked.parse(body, { async: false }) as string;
  }, [body]);

  const thinking = recording?.thinking ?? null;
  const thinkingHtml = useMemo(() => {
    if (!thinking) return '';
    return marked.parse(thinking, { async: false }) as string;
  }, [thinking]);

  useEffect(() => {
    if (!isStreaming) return;
    const el = scrollRef.current;
    if (!el) return;
    // Stick to bottom while streaming.
    el.scrollTop = el.scrollHeight;
  }, [body, isStreaming]);

  if (!recording) {
    return null;
  }

  if (recording.status === 'failed') {
    return (
      <div className="main">
        <div className="main__bar" />
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

  if (!body && (recording.status === 'recording' || recording.status === 'processing' || recording.status === 'stopped')) {
    const heading =
      recording.status === 'recording' ? 'Recording…'
      : recording.status === 'stopped' ? 'Captured.'
      : 'Analyzing…';
    const sub =
      recording.status === 'stopped'
        ? 'Press the send button on the pill to turn this into an instruction set, or the X to discard.'
        : "The instruction set will stream in here as soon as it's ready.";
    return (
      <div className="main">
        <div className="main__bar" />
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
    try {
      await writeText(body);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // Browser fallback for dev mode
      await navigator.clipboard.writeText(body);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    }
  };

  return (
    <div className="main">
      <div className="main__bar">
        <div className="main__actions">
          <button
            className="icon-btn"
            onClick={onCopy}
            disabled={!body}
            aria-label={copied ? 'Copied' : 'Copy'}
            title={copied ? 'Copied' : 'Copy'}
          >
            {copied ? <CheckIcon /> : <CopyIcon />}
          </button>
        </div>
      </div>
      <div className="main__scroll" ref={scrollRef}>
        <div
          className="md"
          dangerouslySetInnerHTML={{
            __html: isStreaming
              ? html + '<span class="streaming-cursor"></span>'
              : html,
          }}
        />
        {!isStreaming && thinking && (
          <details className="thinking">
            <summary className="thinking__summary">
              <ChevronIcon />
              <span>Show thinking</span>
            </summary>
            <div
              className="md thinking__body"
              dangerouslySetInnerHTML={{ __html: thinkingHtml }}
            />
          </details>
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

function CheckIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor"
            strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}


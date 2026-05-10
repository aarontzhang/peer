import { useEffect, useMemo, useRef, useState } from 'react';
import { type Recording, formatRelative, formatDuration } from '@/lib/ipc';
import { firstPlainTextLine, toPlainText } from '@/lib/plainText';

type Props = {
  recording: Recording;
  isPinned: boolean;
  isExpanded: boolean;
  isStreaming: boolean;
  liveBody: string | null;
  liveThinking: string | null;
  onToggleExpand: () => void;
  onTogglePin: () => void;
  onCopy: (text: string) => Promise<void>;
  onDelete: () => void;
  onRetry: () => void;
  retryDisabled?: boolean;
};

export function MessageCard({
  recording,
  isPinned,
  isExpanded,
  isStreaming,
  liveBody,
  liveThinking,
  onToggleExpand,
  onTogglePin,
  onCopy,
  onDelete,
  onRetry,
  retryDisabled,
}: Props) {
  const body = useMemo(
    () => toPlainText(liveBody ?? recording.body ?? ''),
    [liveBody, recording.body],
  );
  const rawThinking = liveThinking ?? recording.thinking ?? null;
  const thinking = useMemo(
    () => (rawThinking ? toPlainText(rawThinking) : null),
    [rawThinking],
  );

  const title =
    firstPlainTextLine(recording.summary ?? recording.body ?? '') ||
    (recording.status === 'processing' ? 'Analyzing…'
      : recording.status === 'recording' ? 'Recording…'
      : recording.status === 'stopped' ? 'Captured (awaiting send)'
      : recording.status === 'canceled' ? 'Cancelled'
      : recording.status === 'failed' ? 'Failed'
      : 'Untitled recording');

  // Typewriter effect for the streaming body.
  const resetKey = `${recording.id}|${recording.status === 'canceled' ? 'C' : 'L'}`;
  const [displayed, setDisplayed] = useState(body);
  const [thinkingOverride, setThinkingOverride] = useState<boolean | null>(null);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | null>(null);

  useEffect(() => {
    setDisplayed(body);
    setThinkingOverride(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetKey]);

  useEffect(() => {
    return () => {
      if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    };
  }, []);

  useEffect(() => {
    if (displayed === body) return;
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

  const onCopyClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!body) return;
    await onCopy(body);
    setCopied(true);
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => {
      setCopied(false);
      copiedTimer.current = null;
    }, 1600);
  };

  const onPinClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onTogglePin();
  };

  const onDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onDelete();
  };

  const onRetryClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onRetry();
  };

  const autoOpenThinking = isStreaming || !body;
  const thinkingOpen = thinkingOverride ?? autoOpenThinking;
  const onToggleThinking = (e: React.MouseEvent) => {
    e.stopPropagation();
    setThinkingOverride(!thinkingOpen);
  };

  const showCopy = !!body;
  const isCanceled = recording.status === 'canceled';
  const isFailed = recording.status === 'failed';

  const onHeaderKey = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onToggleExpand();
    }
  };

  return (
    <div
      className="card"
      data-expanded={isExpanded}
      data-pinned={isPinned}
      data-status={recording.status}
    >
      <div
        role="button"
        tabIndex={0}
        className="card__header"
        onClick={onToggleExpand}
        onKeyDown={onHeaderKey}
        aria-expanded={isExpanded}
      >
        <span className="card__time" aria-hidden>
          {formatRelative(recording.createdAt)}
        </span>
        <span className="card__title">{title}</span>
        <span className="card__duration" aria-hidden>
          {formatDuration(recording.durationMs)}
        </span>
        <span className="card__actions" data-no-drag>
          {isPinned && (
            <span className="card__pin-badge" aria-label="Saved" title="Saved">
              <PinIcon filled />
            </span>
          )}
          {isCanceled && (
            <button
              type="button"
              className="card-icon-btn"
              onClick={onRetryClick}
              disabled={retryDisabled}
              aria-label="Analyze the video"
              title="Analyze the video"
            >
              <RetryIcon />
            </button>
          )}
          {showCopy && (
            <button
              type="button"
              className={`card-icon-btn${copied ? ' card-icon-btn--solid' : ''}`}
              onClick={onCopyClick}
              aria-label={copied ? 'Copied' : 'Copy'}
              title={copied ? 'Copied' : 'Copy'}
            >
              {copied ? <CheckIcon /> : <CopyIcon />}
            </button>
          )}
          <button
            type="button"
            className={`card-icon-btn card-icon-btn--pin${isPinned ? ' card-icon-btn--pinActive' : ''}`}
            onClick={onPinClick}
            aria-label={isPinned ? 'Unsave' : 'Save'}
            aria-pressed={isPinned}
            title={isPinned ? 'Unsave' : 'Save'}
          >
            <PinIcon filled={isPinned} />
          </button>
          <button
            type="button"
            className="card-icon-btn card-icon-btn--danger"
            onClick={onDeleteClick}
            aria-label="Delete recording"
            title="Delete"
          >
            <TrashIcon />
          </button>
        </span>
      </div>
      {isExpanded && (
        <div className="card__body">
          {isFailed ? (
            <p className="card__error">{recording.error ?? 'Unknown error.'}</p>
          ) : isCanceled ? (
            <p className="card__muted">
              You cancelled before analysis. The video is still here — use Retry above
              to analyze it now, or Delete to discard it.
            </p>
          ) : (
            <>
              {thinking && (
                <button
                  type="button"
                  className={`thinking-toggle${thinkingOpen ? ' thinking-toggle--open' : ''}`}
                  onClick={onToggleThinking}
                  aria-expanded={thinkingOpen}
                >
                  <ChevronIcon />
                  <span>{thinkingOpen ? 'Hide thinking' : 'Show thinking'}</span>
                </button>
              )}
              {thinking && thinkingOpen && (
                <div className="thinking__body">{thinking}</div>
              )}
              {thinking && thinkingOpen && body && <hr className="thinking-sep" />}
              {body ? (
                <div className="prompt-body">{displayed}</div>
              ) : (
                <p className="card__muted">
                  {recording.status === 'recording'
                    ? 'Recording…'
                    : recording.status === 'stopped'
                    ? 'Captured. Press Enter on the pill to analyze.'
                    : "Writing the refined prompt…"}
                </p>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function PinIcon({ filled }: { filled: boolean }) {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path
        d="M10.4 1.9 14.1 5.6M11 2.5 7.8 4.3 4.9 4.6 3.4 6.1l6.5 6.5 1.5-1.5.3-2.9 1.8-3.2M5.2 10.8 1.9 14.1"
        fill={filled ? 'currentColor' : 'none'}
        stroke="currentColor"
        strokeWidth="1.3"
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

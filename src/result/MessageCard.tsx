import { useEffect, useMemo, useRef, useState } from 'react';
import { type Recording, formatRelative } from '@/lib/ipc';
import { firstPlainTextLine, toPlainText } from '@/lib/plainText';

type Props = {
  recording: Recording;
  isPinned: boolean;
  isSelected: boolean;
  liveBody: string | null;
  onOpen: () => void;
  onTogglePin: () => void;
  onCopy: (text: string) => Promise<void>;
  onDelete: () => void;
  onRetry: () => void;
  retryDisabled?: boolean;
};

export function MessageCard({
  recording,
  isPinned,
  isSelected,
  liveBody,
  onOpen,
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

  const title =
    firstPlainTextLine(recording.summary ?? recording.body ?? '') ||
    (recording.status === 'processing' ? 'Analyzing…'
      : recording.status === 'recording' ? 'Recording…'
      : recording.status === 'stopped' ? 'Captured (awaiting send)'
      : recording.status === 'canceled' ? 'Cancelled'
      : recording.status === 'failed' ? 'Failed'
      : 'Untitled recording');

  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    };
  }, []);

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

  const showCopy = !!body;
  const isCanceled = recording.status === 'canceled';

  const onHeaderKey = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onOpen();
    }
  };

  return (
    <div
      className="card"
      data-selected={isSelected}
      data-pinned={isPinned}
      data-status={recording.status}
    >
      <div
        role="button"
        tabIndex={0}
        className="card__header"
        onClick={onOpen}
        onKeyDown={onHeaderKey}
      >
        <span className="card__time" aria-hidden>
          {formatRelative(recording.createdAt)}
        </span>
        <span className="card__title">{title}</span>
        <span className="card__actions" data-no-drag>
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

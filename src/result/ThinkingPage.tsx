import { useEffect } from 'react';
import { type Recording, formatRelative, formatDuration } from '@/lib/ipc';
import { firstPlainTextLine, toPlainText } from '@/lib/plainText';

type Props = {
  recording: Recording;
  thinking: string;
  onBack: () => void;
};

export function ThinkingPage({ recording, thinking, onBack }: Props) {
  const title =
    firstPlainTextLine(recording.summary ?? recording.body ?? '') ||
    'Recording';
  const text = toPlainText(thinking);

  useEffect(() => {
    const root = document.querySelector<HTMLElement>('.thinking-page__body');
    root?.focus();
  }, []);

  return (
    <div className="thinking-page" role="dialog" aria-label="Thinking">
      <header className="thinking-page__bar" data-tauri-drag-region>
        <button
          type="button"
          className="thinking-page__back"
          onClick={onBack}
          aria-label="Back"
          data-no-drag
        >
          <BackIcon />
          <span>Back</span>
        </button>
        <div className="thinking-page__heading" data-no-drag>
          <span className="thinking-page__eyebrow">Thinking</span>
          <span className="thinking-page__title">{title}</span>
        </div>
        <div className="thinking-page__meta" data-no-drag>
          <span>{formatRelative(recording.createdAt)}</span>
          <span aria-hidden>·</span>
          <span>{formatDuration(recording.durationMs)}</span>
        </div>
      </header>
      <div className="thinking-page__body" tabIndex={-1}>
        {text ? (
          <div className="thinking-page__text">{text}</div>
        ) : (
          <p className="thinking-page__empty">No thinking captured for this recording.</p>
        )}
      </div>
    </div>
  );
}

function BackIcon() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path
        d="M10 3l-5 5 5 5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

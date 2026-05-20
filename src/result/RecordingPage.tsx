import { useEffect, useMemo, useRef, useState } from 'react';
import { type AutomationEvent, ipc, type Recording } from '@/lib/ipc';
import { firstPlainTextLine, toPlainText } from '@/lib/plainText';

type AutomationState =
  | { kind: 'idle' }
  | { kind: 'running'; label: string; reasoning: string | null; step: number }
  | { kind: 'done'; message: string | null }
  | { kind: 'failed'; message: string }
  | { kind: 'canceled' };

type Props = {
  recording: Recording;
  isPinned: boolean;
  liveBody: string | null;
  liveThinking: string | null;
  onBack: () => void;
  onTogglePin: () => void;
  onCopy: (text: string) => Promise<void>;
  onDelete: () => void;
  onRetry: () => void;
  retryDisabled?: boolean;
};

export function RecordingPage({
  recording,
  isPinned,
  liveBody,
  liveThinking,
  onBack,
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

  // Typewriter effect for the streaming body — same recipe as MessageCard.
  const resetKey = `${recording.id}|${recording.status === 'canceled' ? 'C' : 'L'}`;
  const [displayed, setDisplayed] = useState(body);
  const [copied, setCopied] = useState(false);
  const [thinkingOpen, setThinkingOpen] = useState(false);
  const [automation, setAutomation] = useState<AutomationState>({ kind: 'idle' });
  const [automationError, setAutomationError] = useState<string | null>(null);
  const copiedTimer = useRef<number | null>(null);

  useEffect(() => {
    setDisplayed(body);
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

  useEffect(() => {
    const root = document.querySelector<HTMLElement>('.recording-page__body');
    root?.focus();
  }, []);

  useEffect(() => {
    let active = true;
    const unsub = ipc.onAutomationEvent((e: AutomationEvent) => {
      if (!active) return;
      if (e.id !== recording.id) return;
      switch (e.kind) {
        case 'started':
          setAutomation({ kind: 'running', label: 'Looking at the screen…', reasoning: null, step: 0 });
          setAutomationError(null);
          return;
        case 'step':
          setAutomation({
            kind: 'running',
            label: e.label,
            reasoning: e.reasoning,
            step: e.step,
          });
          return;
        case 'done':
          setAutomation({ kind: 'done', message: e.message });
          return;
        case 'failed':
          setAutomation({ kind: 'failed', message: e.message });
          return;
        case 'canceled':
          setAutomation({ kind: 'canceled' });
          return;
      }
    });
    return () => {
      active = false;
      void unsub.then((fn) => fn());
    };
  }, [recording.id]);

  // Reset the automation banner whenever the user navigates to a different
  // recording — it's per-recording state, not global.
  useEffect(() => {
    setAutomation({ kind: 'idle' });
    setAutomationError(null);
  }, [recording.id]);

  const onCopyClick = async () => {
    if (!body) return;
    await onCopy(body);
    setCopied(true);
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => {
      setCopied(false);
      copiedTimer.current = null;
    }, 1600);
  };

  const showCopy = !!body;
  const isCanceled = recording.status === 'canceled';
  const isFailed = recording.status === 'failed';
  const isProcessing = recording.status === 'processing';
  const isRecording = recording.status === 'recording';
  const isStopped = recording.status === 'stopped';
  const retryButtonDisabled = retryDisabled || isProcessing || isRecording || isStopped;
  const automationRunning = automation.kind === 'running';
  const automationFinishedOk = automation.kind === 'done';
  const canRunAutomation =
    !!body && !isProcessing && !isRecording && !isStopped && !isFailed && !automationRunning;

  const onRunAutomation = async () => {
    if (!canRunAutomation) return;
    setAutomationError(null);
    setAutomation({ kind: 'running', label: 'Starting…', reasoning: null, step: 0 });
    try {
      await ipc.runAutomation(recording.id);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err ?? 'unknown error');
      setAutomation({ kind: 'idle' });
      setAutomationError(msg);
    }
  };

  const onCancelAutomation = async () => {
    try {
      await ipc.cancelAutomation();
    } catch {
      // best-effort; the Rust side just flips a flag
    }
  };

  const automationButtonLabel = automationFinishedOk ? 'Run again' : 'Run automation';

  return (
    <div className="recording-page" role="dialog" aria-label="Recording detail">
      <header className="recording-page__bar" data-tauri-drag-region>
        <button
          type="button"
          className="recording-page__back"
          onClick={onBack}
          aria-label="Back"
          data-no-drag
        >
          <BackIcon />
          <span>Back</span>
        </button>
        <div className="recording-page__heading" data-no-drag>
          <span className="recording-page__title">{title}</span>
        </div>
        <div className="recording-page__actions" data-no-drag>
          <button
            type="button"
            className={`automation-btn${automationRunning ? ' automation-btn--running' : ''}`}
            onClick={automationRunning ? onCancelAutomation : onRunAutomation}
            disabled={!automationRunning && !canRunAutomation}
            aria-label={automationRunning ? 'Cancel automation' : 'Run automation'}
            title={automationRunning ? 'Cancel automation' : 'Run automation'}
          >
            <PlayIcon spinning={automationRunning} />
            <span>{automationRunning ? 'Cancel' : automationButtonLabel}</span>
          </button>
          <button
            type="button"
            className="card-icon-btn"
            onClick={onRetry}
            disabled={retryButtonDisabled}
            aria-label="Re-analyze the recording"
            title="Re-analyze"
          >
            <RetryIcon />
          </button>
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
            onClick={onTogglePin}
            aria-label={isPinned ? 'Unsave' : 'Save'}
            aria-pressed={isPinned}
            title={isPinned ? 'Unsave' : 'Save'}
          >
            <BookmarkIcon filled={isPinned} />
          </button>
          <button
            type="button"
            className="card-icon-btn card-icon-btn--danger"
            onClick={onDelete}
            aria-label="Delete recording"
            title="Delete"
          >
            <TrashIcon />
          </button>
        </div>
      </header>
      <div className="recording-page__body" tabIndex={-1}>
        <div className="recording-page__inner">
          {(automation.kind !== 'idle' || automationError) && (
            <AutomationBanner
              state={automation}
              error={automationError}
              onDismiss={() => {
                setAutomation({ kind: 'idle' });
                setAutomationError(null);
              }}
              onCancel={onCancelAutomation}
            />
          )}
          {isFailed ? (
            <p className="recording-page__error">{recording.error ?? 'Unknown error.'}</p>
          ) : isCanceled && !body ? (
            <p className="recording-page__muted">
              You cancelled before analysis. The video is still here — use Retry above
              to analyze it now, or Delete to discard it.
            </p>
          ) : (
            <>
              {thinking && (
                <>
                  <button
                    type="button"
                    className="thinking-toggle"
                    onClick={() => setThinkingOpen((v) => !v)}
                    aria-expanded={thinkingOpen}
                    aria-controls="thinking-inline"
                  >
                    <ChevronIcon />
                    <span>{thinkingOpen ? 'Hide thinking' : 'Show thinking'}</span>
                  </button>
                  {thinkingOpen && (
                    <div
                      id="thinking-inline"
                      className="thinking-inline"
                      role="region"
                      aria-label="Thinking"
                    >
                      {thinking}
                    </div>
                  )}
                </>
              )}
              {body ? (
                <div className="prompt-body">{displayed}</div>
              ) : (
                <p className="recording-page__muted">
                  {recording.status === 'recording'
                    ? 'Recording…'
                      : recording.status === 'stopped'
                      ? 'Captured. Press Enter on the pill to analyze.'
                      : 'Writing the refined prompt…'}
                  </p>
                )}
              </>
            )}
          </div>
        </div>
    </div>
  );
}

function AutomationBanner({
  state,
  error,
  onDismiss,
  onCancel,
}: {
  state: AutomationState;
  error: string | null;
  onDismiss: () => void;
  onCancel: () => void;
}) {
  if (error) {
    return (
      <div className="automation-banner automation-banner--failed" role="status">
        <div className="automation-banner__title">Automation didn't start</div>
        <div className="automation-banner__detail">{error}</div>
        <div className="automation-banner__actions">
          <button type="button" className="automation-banner__btn" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }
  if (state.kind === 'idle') return null;
  if (state.kind === 'running') {
    return (
      <div className="automation-banner automation-banner--running" role="status" aria-live="polite">
        <div className="automation-banner__title">
          <Spinner />
          <span>Automation running</span>
          {state.step > 0 && <span className="automation-banner__step">step {state.step}</span>}
        </div>
        <div className="automation-banner__detail">{state.label}</div>
        {state.reasoning && (
          <div className="automation-banner__reasoning">{state.reasoning}</div>
        )}
        <div className="automation-banner__actions">
          <button type="button" className="automation-banner__btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    );
  }
  if (state.kind === 'done') {
    return (
      <div className="automation-banner automation-banner--done" role="status">
        <div className="automation-banner__title">Automation finished</div>
        {state.message && <div className="automation-banner__detail">{state.message}</div>}
        <div className="automation-banner__actions">
          <button type="button" className="automation-banner__btn" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }
  if (state.kind === 'canceled') {
    return (
      <div className="automation-banner" role="status">
        <div className="automation-banner__title">Automation canceled</div>
        <div className="automation-banner__actions">
          <button type="button" className="automation-banner__btn" onClick={onDismiss}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="automation-banner automation-banner--failed" role="status">
      <div className="automation-banner__title">Automation failed</div>
      <div className="automation-banner__detail">{state.message}</div>
      <div className="automation-banner__actions">
        <button type="button" className="automation-banner__btn" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}

function Spinner() {
  return (
    <svg
      className="automation-banner__spinner"
      viewBox="0 0 16 16"
      width="13"
      height="13"
      aria-hidden
    >
      <circle
        cx="8"
        cy="8"
        r="6"
        fill="none"
        stroke="currentColor"
        strokeOpacity="0.25"
        strokeWidth="1.6"
      />
      <path
        d="M14 8a6 6 0 0 1-6 6"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

function PlayIcon({ spinning }: { spinning: boolean }) {
  if (spinning) {
    return <Spinner />;
  }
  return (
    <svg viewBox="0 0 16 16" width="11" height="11" aria-hidden>
      <path d="M4.5 3.2 12.5 8 4.5 12.8z" fill="currentColor" />
    </svg>
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

function BookmarkIcon({ filled }: { filled: boolean }) {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path
        d="M4 2.5h8v11l-4-2.7-4 2.7z"
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

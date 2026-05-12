import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { ipc } from '@/lib/ipc';

type Props = {
  recordingId: string;
  /** Disables the input while the initial pipeline stream is still running. */
  disabled?: boolean;
  /** In-flight assistant text, or null when no chat turn is streaming. */
  liveAssistantText: string | null;
  /** Fires the moment the user submits, before any backend round-trip — lets
   *  the parent flip the prompt pane to "Writing the refined prompt…"
   *  instantly instead of waiting on the begin event from Rust. */
  onSendStart?: () => void;
  /** History side-panel state — mirrored into the dock's clock button so it
   *  reads as pressed while the panel is open. */
  historyOpen: boolean;
  onToggleHistory: () => void;
};

export function ChatDock({
  recordingId,
  disabled,
  liveAssistantText,
  onSendStart,
  historyOpen,
  onToggleHistory,
}: Props) {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Surface backend chat errors inline above the input.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    ipc
      .onChatError((e) => {
        if (e.recordingId !== recordingId) return;
        setError(e.message);
        setSending(false);
      })
      .then((u) => {
        unlisten = u;
      });
    return () => {
      unlisten?.();
    };
  }, [recordingId]);

  useEffect(() => {
    if (liveAssistantText === null) {
      setSending(false);
    }
  }, [liveAssistantText]);

  // Auto-grow the textarea up to a soft max.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [draft]);

  const onSubmit = useCallback(async () => {
    const trimmed = draft.trim();
    if (!trimmed || sending || disabled) return;
    setError(null);
    setSending(true);
    setDraft('');
    onSendStart?.();
    try {
      await ipc.sendChatMessage(recordingId, trimmed);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setSending(false);
    }
  }, [draft, sending, disabled, recordingId, onSendStart]);

  return (
    <div className="chat-dock">
      {error && (
        <div className="chat-dock__error" role="status">
          {error}
        </div>
      )}
      <form
        className="chat-dock__input"
        onSubmit={(e) => {
          e.preventDefault();
          void onSubmit();
        }}
      >
        <textarea
          ref={textareaRef}
          className="chat-dock__textarea"
          placeholder={
            disabled
              ? 'Waiting for analysis to finish…'
              : 'Refine the prompt — e.g. "make this shorter" or "fix the second step"'
          }
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void onSubmit();
            }
          }}
          rows={1}
          disabled={disabled || sending}
        />
        <button
          type="button"
          className={`chat-dock__iconBtn${historyOpen ? ' chat-dock__iconBtn--active' : ''}`}
          onClick={onToggleHistory}
          aria-label={historyOpen ? 'Hide history' : 'Show history'}
          aria-pressed={historyOpen}
          title="Prompt history"
        >
          <ClockIcon />
        </button>
        <button
          type="submit"
          className="chat-dock__send"
          disabled={disabled || sending || draft.trim().length === 0}
          aria-label="Send"
          title="Send (Enter)"
        >
          {sending ? <Spinner /> : <SendIcon />}
        </button>
      </form>
    </div>
  );
}

function SendIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path
        d="M2.5 8L13 3.2 10.4 13 7.6 9.3 11.5 5 6.3 8z"
        fill="currentColor"
      />
    </svg>
  );
}

function ClockIcon() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
      <path
        d="M3.2 4.4A6 6 0 1 1 2.5 9"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <path
        d="M1.5 2.5v3h3"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M8 5v3.2l2 1.2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function Spinner() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden className="chat-dock__spinner">
      <circle
        cx="8"
        cy="8"
        r="5.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeDasharray="20 10"
        strokeLinecap="round"
      />
    </svg>
  );
}

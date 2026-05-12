import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { ipc, type RecordingMessage } from '@/lib/ipc';

type Props = {
  recordingId: string;
  /** Disables the input while the initial pipeline stream is still running. */
  disabled?: boolean;
  /** In-flight assistant text, or null when no chat turn is streaming. */
  liveAssistantText: string | null;
  /** Bumped externally when a chat turn completes so the thread re-fetches. */
  refreshKey: number;
  /** Fires the moment the user submits, before any backend round-trip — lets
   *  the parent flip the prompt pane to "Writing the refined prompt…"
   *  instantly instead of waiting on the begin event from Rust. */
  onSendStart?: () => void;
};

export function ChatDock({
  recordingId,
  disabled,
  liveAssistantText,
  refreshKey,
  onSendStart,
}: Props) {
  const [thread, setThread] = useState<RecordingMessage[]>([]);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const messagesRef = useRef<HTMLDivElement | null>(null);
  // Tracks the optimistic user message id so we can dedupe once it arrives
  // back from the persisted thread refresh.
  const optimisticUserRef = useRef<RecordingMessage | null>(null);

  const loadThread = useCallback(async () => {
    try {
      const list = await ipc.getChatThread(recordingId);
      setThread(list);
      // Persisted echo now exists — drop the optimistic copy.
      optimisticUserRef.current = null;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [recordingId]);

  useEffect(() => {
    void loadThread();
  }, [loadThread, refreshKey]);

  // Surface backend chat errors as inline messages instead of swallowing them.
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
      // Stream ended (or never started this mount) → re-enable send.
      setSending(false);
    }
  }, [liveAssistantText]);

  // Keep the message list pinned to the bottom as new content arrives.
  useLayoutEffect(() => {
    const el = messagesRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [thread, liveAssistantText]);

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
    // Optimistic user bubble so the dock feels instant. The persisted echo
    // arrives in the next loadThread() and replaces this entry.
    const optimistic: RecordingMessage = {
      id: `optimistic-${Date.now()}`,
      recordingId,
      createdAt: new Date().toISOString(),
      turnIndex: thread.length,
      role: 'user',
      content: trimmed,
      producedVersionId: null,
    };
    optimisticUserRef.current = optimistic;
    setDraft('');
    onSendStart?.();
    try {
      await ipc.sendChatMessage(recordingId, trimmed);
      // Pull the canonical user row back; assistant message and final body
      // land later via the chat:turn-complete event (see App.tsx).
      await loadThread();
    } catch (err) {
      optimisticUserRef.current = null;
      setError(err instanceof Error ? err.message : String(err));
      setSending(false);
    }
  }, [draft, sending, disabled, recordingId, thread.length, loadThread, onSendStart]);

  const displayedMessages = useMemo(() => {
    if (!optimisticUserRef.current) return thread;
    // If the optimistic row isn't yet present in the fetched thread, show it
    // appended; otherwise the thread already has it.
    const hasIt = thread.some(
      (m) =>
        m.role === 'user' &&
        m.content === optimisticUserRef.current?.content &&
        m.turnIndex >= (optimisticUserRef.current?.turnIndex ?? 0),
    );
    return hasIt ? thread : [...thread, optimisticUserRef.current];
  }, [thread]);

  const showLiveAssistant = liveAssistantText !== null && liveAssistantText.length > 0;

  return (
    <div className="chat-dock">
      {(displayedMessages.length > 0 || showLiveAssistant || error) && (
        <div className="chat-dock__messages" ref={messagesRef}>
          {displayedMessages.map((m) => (
            <div
              key={m.id}
              className={`chat-msg chat-msg--${m.role}`}
            >
              {m.content}
            </div>
          ))}
          {showLiveAssistant && (
            <div className="chat-msg chat-msg--assistant chat-msg--streaming">
              {liveAssistantText}
              <span className="chat-msg__cursor" aria-hidden>▍</span>
            </div>
          )}
          {error && (
            <div className="chat-msg chat-msg--error" role="status">
              {error}
            </div>
          )}
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

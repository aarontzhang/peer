import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ipc, type HotkeyStatus, type Recording } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';
import { toPlainText } from '@/lib/plainText';
import { MessageCard } from './MessageCard';
import { RecordingPage } from './RecordingPage';
import { Settings } from './Settings';
import { ConfirmDialog } from './ConfirmDialog';

async function copyBodyToClipboard(text: string) {
  try {
    await writeText(text);
  } catch {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // best-effort
    }
  }
}

const PINNED_IDS_KEY = 'peer:pinned-recording-ids';
const ACTIVE_TAB_KEY = 'peer:active-tab';

type Tab = 'history' | 'saved';

function getStoredPinnedIds(): Set<string> {
  try {
    const raw = window.localStorage.getItem(PINNED_IDS_KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw);
    return Array.isArray(arr) ? new Set(arr.filter((x) => typeof x === 'string')) : new Set();
  } catch {
    return new Set();
  }
}

function getStoredTab(): Tab {
  const raw = window.localStorage.getItem(ACTIVE_TAB_KEY);
  return raw === 'saved' ? 'saved' : 'history';
}

export function App() {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(getStoredTab);

  // Live streaming buffer per-recording; cleared when stream ends.
  const liveRef = useRef<{ id: string; body: string } | null>(null);
  // Thinking arrives ahead of the streamed prompt and is cached per-recording.
  const liveThinkingRef = useRef<Map<string, string>>(new Map());
  const [, force] = useState(0);
  const triggerRender = useCallback(() => force((n) => n + 1), []);

  // Tracks the most recent recording id we've auto-jumped to. The pill emits
  // a `recording` event on every elapsed-time tick; without this we'd reset
  // the user's manual expansion on every tick.
  const autoSelectedRecRef = useRef<string | null>(null);

  const [showSettings, setShowSettings] = useState(false);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [pinnedIds, setPinnedIds] = useState<Set<string>>(getStoredPinnedIds);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [detailForId, setDetailForId] = useState<string | null>(null);

  useEffect(() => {
    window.localStorage.setItem(
      PINNED_IDS_KEY,
      JSON.stringify(Array.from(pinnedIds)),
    );
  }, [pinnedIds]);

  useEffect(() => {
    window.localStorage.setItem(ACTIVE_TAB_KEY, tab);
  }, [tab]);

  const togglePin = useCallback((id: string) => {
    setPinnedIds((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  // Prune pinned ids that no longer exist in the recordings list.
  useEffect(() => {
    setPinnedIds((cur) => {
      if (cur.size === 0) return cur;
      const known = new Set(recordings.map((r) => r.id));
      let changed = false;
      const next = new Set<string>();
      for (const id of cur) {
        if (known.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : cur;
    });
  }, [recordings]);

  const refreshList = useCallback(async () => {
    const list = await ipc.listRecordings();
    setRecordings(list);
    setExpandedId((cur) => {
      if (cur && list.some((r) => r.id === cur)) return cur;
      return list[0]?.id ?? null;
    });
  }, []);

  useEffect(() => {
    void refreshList();
    void ipc.getHotkeyStatus().then(setHotkey);
    const unsub = ipc.onHotkeyStatus(setHotkey);
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList]);

  // Pill events drive list refreshes.
  useEffect(() => {
    const unsub = ipc.onPillEvent((e) => {
      if (
        e.kind === 'recording' || e.kind === 'stopped' ||
        e.kind === 'processing' || e.kind === 'done' || e.kind === 'error' ||
        e.kind === 'idle'
      ) {
        void refreshList();
      }
      // Auto-expand a fresh recording — but only on the first tick of a new
      // session so manual clicks aren't yanked back on subsequent ticks. Also
      // hop the user back to History so they see the live entry instead of
      // staring at a stale Saved view.
      if (e.kind === 'recording' && autoSelectedRecRef.current !== e.id) {
        autoSelectedRecRef.current = e.id;
        setExpandedId(e.id);
        setTab('history');
      }
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList]);

  useEffect(() => {
    const unsub = ipc.onThinking((t) => {
      liveThinkingRef.current.set(t.id, t.thinking);
      triggerRender();
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [triggerRender]);

  useEffect(() => {
    const unsub = ipc.onResultChunk((c) => {
      if (c.kind === 'begin') {
        liveRef.current = { id: c.id, body: '' };
        setExpandedId(c.id);
        triggerRender();
        return;
      }
      if (c.kind === 'delta') {
        if (!liveRef.current || liveRef.current.id !== c.id) {
          liveRef.current = { id: c.id, body: '' };
        }
        liveRef.current.body += c.text;
        triggerRender();
        return;
      }
      if (c.kind === 'end') {
        liveRef.current = { id: c.id, body: c.text };
        triggerRender();
        void refreshList();
        if (c.text) void copyBodyToClipboard(c.text);
      }
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList, triggerRender]);

  // History shows everything, newest first (already the storage order).
  // Saved filters down to pinned items, preserving the same chronology.
  const visibleRecordings = useMemo(() => {
    if (tab === 'saved') return recordings.filter((r) => pinnedIds.has(r.id));
    return recordings;
  }, [recordings, pinnedIds, tab]);

  // Esc closes the recording detail overlay.
  useGlobalKey('Escape', (e) => {
    if (detailForId !== null) {
      e.preventDefault();
      setDetailForId(null);
    }
  });

  // ↑/↓ navigate the visible list (highlight prev/next card).
  useGlobalKey(['ArrowDown', 'ArrowUp'], (e) => {
    if (showSettings || detailForId !== null) return;
    if (visibleRecordings.length === 0) return;
    const idx = visibleRecordings.findIndex((r) => r.id === expandedId);
    const next = e.key === 'ArrowDown'
      ? Math.min(visibleRecordings.length - 1, (idx < 0 ? 0 : idx + 1))
      : Math.max(0, (idx <= 0 ? 0 : idx - 1));
    setExpandedId(visibleRecordings[next].id);
    e.preventDefault();
  });

  // Stopped recordings are waiting on the pill review choice. Let keyboard
  // confirmation mirror those two pill buttons.
  useGlobalKey(['Enter', 'Delete', 'Backspace'], (e) => {
    if (showSettings || isEditableTarget(e.target)) return;
    const expanded = recordings.find((r) => r.id === expandedId) ?? null;
    if (expanded?.status !== 'stopped') return;

    if (e.key === 'Enter') {
      e.preventDefault();
      void ipc.sendRecording();
      return;
    }

    e.preventDefault();
    void ipc.cancelRecording();
  });

  // ⌘C copies the visible prompt body when no text selection is active.
  useGlobalKey('c', (e) => {
    if (!(e.metaKey || e.ctrlKey)) return;
    const sel = window.getSelection?.();
    if (sel && sel.toString().length > 0) return;
    const expanded = recordings.find((r) => r.id === expandedId) ?? null;
    const liveBody = liveRef.current && liveRef.current.id === expandedId
      ? liveRef.current.body : null;
    const visible = liveBody ?? expanded?.body;
    if (visible) {
      e.preventDefault();
      void copyBodyToClipboard(toPlainText(visible));
    }
  });

  const showHotkeyWarning = hotkey !== null && !hotkey.installed;

  const detailOpen = detailForId !== null;

  return (
    <div className="app">
      {showHotkeyWarning && !detailOpen && (
        <div className="hotkey-banner" role="status">
          <span className="hotkey-banner__dot" aria-hidden />
          <span className="hotkey-banner__msg">
            <strong>{hotkey?.label ?? 'Recording'} hotkey unavailable.</strong>{' '}
            {hotkey?.reason ?? 'Grant Peer Accessibility access in System Settings → Privacy & Security → Accessibility, then quit and reopen Peer.'}
          </span>
        </div>
      )}
      {!detailOpen && (
        <header className="topbar" data-tauri-drag-region>
          <div className="topbar__brand">
            <BrandMark />
            <span className="topbar__brandName">Peer</span>
          </div>
          <nav className="topbar__tabs" role="tablist" data-no-drag>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'history'}
              className={`tab${tab === 'history' ? ' tab--active' : ''}`}
              onClick={() => setTab('history')}
            >
              <span className="tab__icon" aria-hidden><HistoryIcon /></span>
              <span>History</span>
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'saved'}
              className={`tab${tab === 'saved' ? ' tab--active' : ''}`}
              onClick={() => setTab('saved')}
            >
              <span className="tab__icon" aria-hidden><SavedIcon /></span>
              <span>Saved</span>
            </button>
          </nav>
          <div className="topbar__actions" data-no-drag>
            <button
              type="button"
              className="topbar__gear"
              onClick={() => setShowSettings(true)}
              aria-label="Open settings"
              title="Settings"
            >
              <GearIcon />
            </button>
          </div>
        </header>
      )}
      {!detailOpen && (
        <main className="feed" role="list" aria-label="Recordings">
          {visibleRecordings.length === 0 ? (
            <FeedEmpty tab={tab} />
          ) : (
            visibleRecordings.map((rec) => {
              const liveBody = liveRef.current && liveRef.current.id === rec.id
                ? liveRef.current.body
                : null;
              return (
                <MessageCard
                  key={rec.id}
                  recording={rec}
                  isPinned={pinnedIds.has(rec.id)}
                  isSelected={expandedId === rec.id}
                  liveBody={liveBody}
                  onOpen={() => {
                    setExpandedId(rec.id);
                    setDetailForId(rec.id);
                  }}
                  onTogglePin={() => togglePin(rec.id)}
                  onCopy={copyBodyToClipboard}
                  onDelete={() => setPendingDeleteId(rec.id)}
                  onRetry={async () => {
                    if (retrying) return;
                    setRetrying(true);
                    try {
                      await ipc.retryRecording(rec.id);
                      await refreshList();
                    } finally {
                      setRetrying(false);
                    }
                  }}
                  retryDisabled={retrying}
                />
              );
            })
          )}
        </main>
      )}
      {detailForId !== null && (() => {
        const rec = recordings.find((r) => r.id === detailForId) ?? null;
        if (!rec) return null;
        const liveBody = liveRef.current && liveRef.current.id === rec.id
          ? liveRef.current.body
          : null;
        const liveThinking = liveThinkingRef.current.get(rec.id) ?? null;
        return (
          <RecordingPage
            recording={rec}
            isPinned={pinnedIds.has(rec.id)}
            liveBody={liveBody}
            liveThinking={liveThinking}
            onBack={() => setDetailForId(null)}
            onTogglePin={() => togglePin(rec.id)}
            onCopy={copyBodyToClipboard}
            onDelete={() => setPendingDeleteId(rec.id)}
            onRetry={async () => {
              if (retrying) return;
              setRetrying(true);
              try {
                await ipc.retryRecording(rec.id);
                await refreshList();
              } finally {
                setRetrying(false);
              }
            }}
            retryDisabled={retrying}
          />
        );
      })()}
      <Settings
        open={showSettings}
        onClose={() => setShowSettings(false)}
      />
      <ConfirmDialog
        open={pendingDeleteId !== null}
        title="Delete recording?"
        message="This recording and its transcript will be permanently removed. This can't be undone."
        confirmLabel={deleting ? 'Deleting…' : 'Delete'}
        confirmDestructive
        busy={deleting}
        onCancel={() => {
          if (!deleting) setPendingDeleteId(null);
        }}
        onConfirm={async () => {
          if (!pendingDeleteId || deleting) return;
          setDeleting(true);
          try {
            await ipc.deleteRecording(pendingDeleteId);
            if (detailForId === pendingDeleteId) setDetailForId(null);
            setPendingDeleteId(null);
            await refreshList();
          } finally {
            setDeleting(false);
          }
        }}
      />
    </div>
  );
}

function FeedEmpty({ tab }: { tab: Tab }) {
  if (tab === 'saved') {
    return (
      <div className="feed-empty">
        <div className="feed-empty__title">Nothing saved yet.</div>
        <div className="feed-empty__sub">
          Hover a recording in History and click the pin to keep it here.
        </div>
      </div>
    );
  }
  return (
    <div className="feed-empty">
      <div className="feed-empty__title">Show, don't tell.</div>
      <div className="feed-empty__sub">
        Click the orb on the floating pill to start recording. Peer turns your
        screen + narration into a paste-ready instruction set for Claude Code.
      </div>
    </div>
  );
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return target.isContentEditable || tag === 'input' || tag === 'textarea' || tag === 'select';
}

/** Face + round glasses — same brand mark used elsewhere in the app. */
function BrandMark() {
  const maskId = 'topbar-brand-head-mask';
  const [dx, dy] = useRandomGaze();
  const gazeTransform = `translate(${dx}px, ${dy}px)`;
  return (
    <svg
      viewBox="-50 -50 100 100"
      aria-hidden
      className="topbar__brandOrb"
    >
      <defs>
        <mask id={maskId} maskUnits="userSpaceOnUse" x="-50" y="-50" width="100" height="100">
          <rect x="-50" y="-50" width="100" height="100" fill="white" />
          <g style={{ transform: gazeTransform }} fill="black">
            <circle cx="-15" cy="0" r="12.5" />
            <circle cx="15" cy="0" r="12.5" />
          </g>
        </mask>
      </defs>
      <g fill="none" stroke="currentColor" strokeLinecap="round">
        <circle cx="0" cy="0" r="37" strokeWidth="5" mask={`url(#${maskId})`} />
        <g strokeWidth="4" style={{ transform: gazeTransform }}>
          <circle cx="-15" cy="0" r="10" />
          <circle cx="15"  cy="0" r="10" />
          <line x1="-5" y1="0" x2="5" y2="0" />
        </g>
      </g>
    </svg>
  );
}

const GAZE_MAX = 16;
const GAZE_LERP = 0.16;
const GAZE_EPSILON = 0.05;
const HOLD_MIN_MS = 1400;
const HOLD_MAX_MS = 3200;
const REST_PROBABILITY = 0.25;

function useRandomGaze(): [number, number] {
  const [gaze, setGaze] = useState<[number, number]>([0, 0]);
  const target = useRef<[number, number]>([0, 0]);
  const current = useRef<[number, number]>([0, 0]);

  useEffect(() => {
    let timer: number | undefined;
    const pick = () => {
      if (Math.random() < REST_PROBABILITY) {
        target.current = [0, 0];
      } else {
        const angle = Math.random() * Math.PI * 2;
        const r = GAZE_MAX * (0.55 + Math.random() * 0.45);
        target.current = [Math.cos(angle) * r, Math.sin(angle) * r];
      }
      const hold = HOLD_MIN_MS + Math.random() * (HOLD_MAX_MS - HOLD_MIN_MS);
      timer = window.setTimeout(pick, hold);
    };
    pick();
    return () => { if (timer !== undefined) window.clearTimeout(timer); };
  }, []);

  useEffect(() => {
    let raf = 0;
    let stopped = false;
    const step = () => {
      const [tx, ty] = target.current;
      const [cx, cy] = current.current;
      const nx = cx + (tx - cx) * GAZE_LERP;
      const ny = cy + (ty - cy) * GAZE_LERP;
      current.current = [nx, ny];
      const settledX = Math.abs(nx - tx) < GAZE_EPSILON;
      const settledY = Math.abs(ny - ty) < GAZE_EPSILON;
      setGaze([settledX ? tx : nx, settledY ? ty : ny]);
      if (!stopped) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => { stopped = true; cancelAnimationFrame(raf); };
  }, []);

  return gaze;
}

/** Clock with a counter-clockwise rewind arrow — symbolizes time travel
 *  back through past recordings. */
function HistoryIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
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

/** Push-pin glyph — the same shape used on the per-card save toggle, so the
 *  Saved tab visually echoes the action that fills it. */
function SavedIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <path
        d="M10.4 1.9 14.1 5.6M11 2.5 7.8 4.3 4.9 4.6 3.4 6.1l6.5 6.5 1.5-1.5.3-2.9 1.8-3.2M5.2 10.8 1.9 14.1"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden>
      <path
        d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

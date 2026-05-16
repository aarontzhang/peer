import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ipc, type HotkeyStatus, type Recording } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';
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
  const [tab, setTab] = useState<Tab>(getStoredTab);

  // Live streaming buffer per-recording; cleared when stream ends.
  const liveRef = useRef<{ id: string; body: string } | null>(null);
  // Thinking arrives ahead of the streamed prompt and is cached per-recording.
  const liveThinkingRef = useRef<Map<string, string>>(new Map());
  const [, force] = useState(0);
  const triggerRender = useCallback(() => force((n) => n + 1), []);

  // Tracks the most recent recording id we've auto-jumped tabs for.
  const autoTabSwitchedRecRef = useRef<string | null>(null);

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

  // Prune pinned ids that no longer exist in the recordings list. Skip while
  // the list is empty — that's the pre-load state, not a real "no recordings"
  // signal, and pruning then would wipe every saved id from localStorage.
  useEffect(() => {
    if (recordings.length === 0) return;
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
      // Hop the user back to History when a brand-new recording starts so
      // they see the live entry instead of staring at a stale Saved view.
      // Only on the first tick of a new session — the pill emits this on
      // every elapsed-time update.
      if (e.kind === 'recording' && autoTabSwitchedRecRef.current !== e.id) {
        autoTabSwitchedRecRef.current = e.id;
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
                  liveBody={liveBody}
                  onOpen={() => setDetailForId(rec.id)}
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
        <div className="feed-empty__title">
          <span className="feed-empty__accent">Save</span> what you'll run again.
        </div>
        <div className="feed-empty__sub">
          Bookmark a recording in History and it lives here — ready to hand to
          an agent the next time you need the same workflow.
        </div>
        <FlowDiagram variant="saved" />
      </div>
    );
  }
  return (
    <div className="feed-empty">
      <div className="feed-empty__title">
        <span className="feed-empty__accent">Show</span>, don't tell.
      </div>
      <div className="feed-empty__sub">
        Record once with the floating pill. Peer turns your screen and
        narration into a repeatable instruction set any agent can replay.
      </div>
      <FlowDiagram variant="history" />
    </div>
  );
}

/** Three connected nodes that mirror the recording → instructions → replay
 *  loop. The accented node is whichever action the user needs to take next:
 *  the pill (history) or the bookmark (saved). No labels — the glyphs map to
 *  surfaces the user will recognize in the app. */
function FlowDiagram({ variant }: { variant: 'history' | 'saved' }) {
  const ACCENT = 'var(--color-accent)';
  const MUTED = 'var(--color-fg-dim)';
  const RING = 'var(--color-line-strong)';

  const nodeCenters = [40, 130, 220];
  const radius = 26;

  return (
    <svg
      className="feed-empty__flow"
      viewBox="0 0 260 64"
      role="img"
      aria-label={
        variant === 'history'
          ? 'Recording flow: pill captures, instructions are written, agent replays'
          : 'Saved flow: a recording is bookmarked, then handed to an agent'
      }
    >
      {/* Connector arrows between the three nodes. */}
      {[0, 1].map((i) => {
        const x1 = nodeCenters[i] + radius + 2;
        const x2 = nodeCenters[i + 1] - radius - 2;
        return (
          <g key={i} stroke={MUTED} strokeWidth="1.2" fill="none" strokeLinecap="round">
            <line x1={x1} y1="32" x2={x2 - 4} y2="32" />
            <polyline points={`${x2 - 6},29 ${x2 - 2},32 ${x2 - 6},35`} strokeLinejoin="round" />
          </g>
        );
      })}

      {/* Three nodes. The accented index marks the action the user takes. */}
      {nodeCenters.map((cx, i) => {
        const accentIdx = variant === 'history' ? 0 : 1;
        const isAccent = i === accentIdx;
        return (
          <g key={i}>
            <circle
              cx={cx}
              cy="32"
              r={radius}
              fill="none"
              stroke={isAccent ? ACCENT : RING}
              strokeWidth={isAccent ? 1.5 : 1.1}
            />
            <g transform={`translate(${cx}, 32)`} stroke={isAccent ? ACCENT : MUTED} fill="none" strokeLinecap="round" strokeLinejoin="round">
              {variant === 'history' && i === 0 && <NodePill />}
              {variant === 'history' && i === 1 && <NodeDoc />}
              {variant === 'history' && i === 2 && <NodePlay filled={isAccent} />}
              {variant === 'saved' && i === 0 && <NodeDoc />}
              {variant === 'saved' && i === 1 && <NodeBookmark filled={isAccent} />}
              {variant === 'saved' && i === 2 && <NodePlay filled={false} />}
            </g>
          </g>
        );
      })}
    </svg>
  );
}

/** Mini silhouette of the floating pill window: a tall rounded rect with the
 *  brand face dot at its head. Tells the user which surface in the app actually
 *  starts a recording. */
function NodePill() {
  return (
    <g strokeWidth="1.3">
      <rect x="-5" y="-12" width="10" height="24" rx="5" />
      <circle cx="0" cy="-6" r="2.2" fill="currentColor" stroke="none" />
    </g>
  );
}

function NodeDoc() {
  return (
    <g strokeWidth="1.3">
      <rect x="-8" y="-9" width="16" height="18" rx="2" />
      <line x1="-5" y1="-4" x2="5" y2="-4" />
      <line x1="-5" y1="0"  x2="5" y2="0"  />
      <line x1="-5" y1="4"  x2="2" y2="4"  />
    </g>
  );
}

function NodeBookmark({ filled }: { filled: boolean }) {
  return (
    <g strokeWidth="1.4">
      <path
        d="M-6 -10 L6 -10 L6 11 L0 6 L-6 11 Z"
        fill={filled ? 'currentColor' : 'none'}
      />
    </g>
  );
}

function NodePlay({ filled }: { filled: boolean }) {
  return (
    <g strokeWidth="1.3">
      <path
        d="M-5 -8 L8 0 L-5 8 Z"
        fill={filled ? 'currentColor' : 'none'}
      />
    </g>
  );
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
            <circle cx="-15" cy="0" r="7.25" />
            <circle cx="15" cy="0" r="7.25" />
          </g>
        </mask>
      </defs>
      <g fill="none" stroke="currentColor" strokeLinecap="round">
        <g strokeWidth="5.5" style={{ transform: gazeTransform }}>
          <circle cx="-15" cy="0" r="10" />
          <circle cx="15"  cy="0" r="10" />
          <line x1="-5" y1="0" x2="5" y2="0" />
        </g>
        <circle cx="0" cy="0" r="37" strokeWidth="6.5" mask={`url(#${maskId})`} />
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

/** Bookmark glyph — matches the per-card save toggle so the Saved tab
 *  visually echoes the action that fills it. */
function SavedIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <path
        d="M4 2.5h8v11l-4-2.7-4 2.7z"
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

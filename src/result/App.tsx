import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import {
  ipc,
  type AccountStatus,
  type AuthChangedPayload,
  type HotkeyStatus,
  type Recording,
} from '@/lib/ipc';
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
  const [account, setAccount] = useState<AccountStatus | null>(null);
  const [pendingSignIn, setPendingSignIn] = useState(false);
  const [signInError, setSignInError] = useState<string | null>(null);
  const [noAccount, setNoAccount] = useState(false);
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
    void ipc.getSession()
      .then(setAccount)
      .catch(() => setAccount({ signedIn: false, email: null }));
    void ipc.getHotkeyStatus().then(setHotkey);
    const unsub = ipc.onHotkeyStatus(setHotkey);
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    ipc
      .onAuthChanged(async (payload: AuthChangedPayload) => {
        setAccount(await ipc.getSession());
        setPendingSignIn(false);
        if (payload.reason === 'no_account') {
          setNoAccount(true);
          setSignInError(null);
        } else if (payload.error) {
          setSignInError(payload.error);
          setNoAccount(false);
        } else {
          setSignInError(null);
          setNoAccount(false);
        }
      })
      .then((u) => {
        unlisten = u;
      });
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!pendingSignIn) return;

    const timeout = window.setTimeout(() => {
      setPendingSignIn(false);
      setSignInError('Sign-in timed out. Try again.');
    }, 10 * 60 * 1000);

    return () => window.clearTimeout(timeout);
  }, [pendingSignIn]);

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
  const signedIn = account?.signedIn === true;
  const signedOut = account?.signedIn === false;

  const onLogin = async () => {
    setSignInError(null);
    setNoAccount(false);
    setPendingSignIn(true);
    try {
      await ipc.startGoogleSignIn();
    } catch (err) {
      setPendingSignIn(false);
      setSignInError(err instanceof Error ? err.message : String(err));
    }
  };

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
          {signedIn && (
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
          )}
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
      {!detailOpen && signedOut && (
        <AuthGate
          pendingSignIn={pendingSignIn}
          signInError={signInError}
          noAccount={noAccount}
          onSignIn={onLogin}
          onUseDifferentAccount={() => {
            setNoAccount(false);
            setSignInError(null);
          }}
        />
      )}
      {!detailOpen && signedIn && (
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
      {signedIn && detailForId !== null && (() => {
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

function AuthGate({
  pendingSignIn,
  signInError,
  noAccount,
  onSignIn,
  onUseDifferentAccount,
}: {
  pendingSignIn: boolean;
  signInError: string | null;
  noAccount: boolean;
  onSignIn: () => void;
  onUseDifferentAccount: () => void;
}) {
  return (
    <main className="auth-gate" aria-label="Sign in">
      <section className="auth-gate__panel">
        <div className="auth-gate__mark" aria-hidden>
          <BrandMark />
        </div>
        <div className="auth-gate__copy">
          <h1>
            <span>Sign in,</span> then record.
          </h1>
          <p>
            Peer needs your beta account before it can record, analyze, or save
            a workflow.
          </p>
        </div>

        <div className="auth-steps" aria-label="Peer setup steps">
          <div className="auth-step">
            <span className="auth-step__num">1</span>
            <strong>Sign in with Google.</strong>
          </div>
          <div className="auth-step">
            <span className="auth-step__num">2</span>
            <strong>Record with the floating pill.</strong>
          </div>
          <div className="auth-step">
            <span className="auth-step__num">3</span>
            <strong>Send it to generate the workflow.</strong>
          </div>
        </div>

        <div className="auth-gate__actions">
          {noAccount ? (
            <button className="btn btn--neutralLight auth-gate__button" type="button" onClick={onUseDifferentAccount}>
              Use a different account
            </button>
          ) : (
            <button
              className="btn btn--primary auth-gate__button"
              type="button"
              onClick={onSignIn}
              disabled={pendingSignIn}
            >
              {pendingSignIn ? 'Waiting for browser...' : 'Sign in'}
            </button>
          )}
        </div>

        {pendingSignIn && !noAccount && (
          <p className="auth-gate__hint">Complete sign-in in your browser.</p>
        )}
        {noAccount && (
          <p className="auth-gate__hint">
            This Google account is not on the beta yet.
          </p>
        )}
        {signInError && !noAccount && (
          <p className="auth-gate__error">{signInError}</p>
        )}
      </section>
    </main>
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
          Bookmark a recording in History and it lives here, ready to hand to an
          agent the next time you need the same workflow.
        </div>
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
        narration into a repeatable workflow any agent can replay.
      </div>
    </div>
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

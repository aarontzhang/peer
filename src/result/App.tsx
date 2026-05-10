import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent, PointerEvent as ReactPointerEvent } from 'react';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ipc, type HotkeyStatus, type Recording } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';
import { toPlainText } from '@/lib/plainText';
import { HistorySidebar } from './HistorySidebar';
import { ResultView } from './ResultView';
import { EmptyState } from './EmptyState';
import { Settings } from './Settings';
import { SidebarAccount } from './SidebarAccount';
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

const SIDEBAR_WIDTH_KEY = 'peer:result-sidebar-width';
const PINNED_IDS_KEY = 'peer:pinned-recording-ids';

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

const SIDEBAR_DEFAULT_WIDTH = 218;
const SIDEBAR_MIN_WIDTH = 176;
const SIDEBAR_MAX_WIDTH = 420;
const MAIN_MIN_WIDTH = 360;

function clampSidebarWidth(width: number, maxWidth = SIDEBAR_MAX_WIDTH) {
  return Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), maxWidth);
}

function getStoredSidebarWidth() {
  const raw = window.localStorage.getItem(SIDEBAR_WIDTH_KEY);
  const width = raw ? Number.parseInt(raw, 10) : SIDEBAR_DEFAULT_WIDTH;
  return Number.isFinite(width) ? clampSidebarWidth(width) : SIDEBAR_DEFAULT_WIDTH;
}

export function App() {
  const appRef = useRef<HTMLDivElement>(null);
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(getStoredSidebarWidth);

  // Live streaming buffer per-recording; cleared when stream ends.
  const liveRef = useRef<{ id: string; body: string } | null>(null);
  // Thinking arrives ahead of the streamed prompt and is cached per-recording
  // so switching to another row and back doesn't drop it.
  const liveThinkingRef = useRef<Map<string, string>>(new Map());
  const [, force] = useState(0);
  const triggerRender = useCallback(() => force((n) => n + 1), []);

  // Tracks the most recent recording id we've auto-jumped to. The pill emits
  // a `recording` event on every elapsed-time tick; without this we'd reset
  // the user's manual sidebar selection on every tick.
  const autoSelectedRecRef = useRef<string | null>(null);

  const [streamingId, setStreamingId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [pinnedIds, setPinnedIds] = useState<Set<string>>(getStoredPinnedIds);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [retrying, setRetrying] = useState(false);

  useEffect(() => {
    window.localStorage.setItem(
      PINNED_IDS_KEY,
      JSON.stringify(Array.from(pinnedIds)),
    );
  }, [pinnedIds]);

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

  const getMaxSidebarWidth = useCallback(() => {
    const appWidth = appRef.current?.clientWidth ?? window.innerWidth;
    return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, appWidth - MAIN_MIN_WIDTH));
  }, []);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_WIDTH_KEY, String(Math.round(sidebarWidth)));
  }, [sidebarWidth]);

  useEffect(() => {
    const syncWidthToWindow = () => {
      setSidebarWidth((width) => clampSidebarWidth(width, getMaxSidebarWidth()));
    };

    syncWidthToWindow();
    window.addEventListener('resize', syncWidthToWindow);
    return () => window.removeEventListener('resize', syncWidthToWindow);
  }, [getMaxSidebarWidth]);

  const resizeSidebarBy = useCallback((delta: number) => {
    setSidebarWidth((width) => clampSidebarWidth(width + delta, getMaxSidebarWidth()));
  }, [getMaxSidebarWidth]);

  const onSidebarResizePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;

    event.preventDefault();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const onPointerMove = (moveEvent: globalThis.PointerEvent) => {
      const nextWidth = startWidth + moveEvent.clientX - startX;
      setSidebarWidth(clampSidebarWidth(nextWidth, getMaxSidebarWidth()));
    };

    const onPointerUp = () => {
      document.removeEventListener('pointermove', onPointerMove);
      document.removeEventListener('pointerup', onPointerUp);
      document.removeEventListener('pointercancel', onPointerUp);
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
    };

    document.addEventListener('pointermove', onPointerMove);
    document.addEventListener('pointerup', onPointerUp, { once: true });
    document.addEventListener('pointercancel', onPointerUp, { once: true });
  }, [getMaxSidebarWidth, sidebarWidth]);

  const onSidebarResizeKeyDown = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const step = event.shiftKey ? 32 : 12;
    resizeSidebarBy(event.key === 'ArrowRight' ? step : -step);
  }, [resizeSidebarBy]);

  const refreshList = useCallback(async () => {
    const list = await ipc.listRecordings();
    setRecordings(list);
    setSelectedId((cur) => {
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
      // Auto-jump to a recording only on the first tick of a new session,
      // so manual sidebar clicks aren't yanked back on subsequent ticks.
      if (e.kind === 'recording' && autoSelectedRecRef.current !== e.id) {
        autoSelectedRecRef.current = e.id;
        setSelectedId(e.id);
      }
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList]);

  // Thinking arrives once per-window analyses finish, before the prompt
  // streams. Stash it so ResultView can show it above the streaming body.
  useEffect(() => {
    const unsub = ipc.onThinking((t) => {
      liveThinkingRef.current.set(t.id, t.thinking);
      triggerRender();
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [triggerRender]);

  // Streaming chunks.
  useEffect(() => {
    const unsub = ipc.onResultChunk((c) => {
      if (c.kind === 'begin') {
        liveRef.current = { id: c.id, body: '' };
        setStreamingId(c.id);
        // Don't force the selection — if the user is browsing other
        // history entries, leave them where they are. The streaming
        // result is still accessible by clicking its row.
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
        setStreamingId(null);
        triggerRender();
        void refreshList();
        // Auto-copy the finished prompt so it's ready to paste into Claude Code.
        if (c.text) void copyBodyToClipboard(c.text);
      }
    });
    return () => { void unsub.then((fn) => fn()); };
  }, [refreshList, triggerRender]);

  // Recordings, pinned-first, preserving creation-time order within each group.
  // This is the order shown in the sidebar; keyboard nav follows the same.
  const orderedRecordings = useMemo(() => {
    if (pinnedIds.size === 0) return recordings;
    const pinned: Recording[] = [];
    const rest: Recording[] = [];
    for (const r of recordings) {
      if (pinnedIds.has(r.id)) pinned.push(r);
      else rest.push(r);
    }
    return [...pinned, ...rest];
  }, [recordings, pinnedIds]);

  // ↑/↓ navigate the sidebar.
  useGlobalKey(['ArrowDown', 'ArrowUp'], (e) => {
    if (showSettings) return;
    if (orderedRecordings.length === 0) return;
    const idx = orderedRecordings.findIndex((r) => r.id === selectedId);
    const next = e.key === 'ArrowDown'
      ? Math.min(orderedRecordings.length - 1, (idx < 0 ? 0 : idx + 1))
      : Math.max(0, (idx <= 0 ? 0 : idx - 1));
    setSelectedId(orderedRecordings[next].id);
    e.preventDefault();
  });

  // Stopped recordings are waiting on the pill review choice. Let keyboard
  // confirmation mirror those two pill buttons.
  useGlobalKey(['Enter', 'Delete', 'Backspace'], (e) => {
    if (showSettings || isEditableTarget(e.target)) return;
    if (selected?.status !== 'stopped') return;

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
    if (sel && sel.toString().length > 0) return; // user selected text → default copy
    const visible = liveBody ?? selected?.body;
    if (visible) {
      e.preventDefault();
      void copyBodyToClipboard(toPlainText(visible));
    }
  });

  const selected = useMemo(
    () => recordings.find((r) => r.id === selectedId) ?? null,
    [recordings, selectedId],
  );

  const liveBody = liveRef.current && liveRef.current.id === selectedId
    ? liveRef.current.body
    : null;

  const liveThinking = selectedId ? liveThinkingRef.current.get(selectedId) ?? null : null;

  const hasContent = !!selected;

  const showHotkeyWarning = hotkey !== null && !hotkey.installed;

  return (
    <div
      ref={appRef}
      className="app"
      style={{ '--sidebar-width': `${Math.round(sidebarWidth)}px` } as CSSProperties}
    >
      {showHotkeyWarning && (
        <div className="hotkey-banner" role="status">
          <span className="hotkey-banner__dot" aria-hidden />
          <span className="hotkey-banner__msg">
            <strong>{hotkey?.label ?? 'Recording'} hotkey unavailable.</strong>{' '}
            {hotkey?.reason ?? 'Grant Peer Accessibility access in System Settings → Privacy & Security → Accessibility, then quit and reopen Peer.'}
          </span>
        </div>
      )}
      <HistorySidebar
        items={recordings}
        selectedId={selectedId}
        pinnedIds={pinnedIds}
        onSelect={setSelectedId}
        onTogglePin={togglePin}
        footer={<SidebarAccount onOpenSettings={() => setShowSettings(true)} />}
      />
      <div
        className="sidebar-resizer"
        role="separator"
        aria-label="Resize history sidebar"
        aria-orientation="vertical"
        aria-valuemin={SIDEBAR_MIN_WIDTH}
        aria-valuemax={getMaxSidebarWidth()}
        aria-valuenow={Math.round(sidebarWidth)}
        tabIndex={0}
        data-no-drag
        onPointerDown={onSidebarResizePointerDown}
        onKeyDown={onSidebarResizeKeyDown}
      />
      {hasContent ? (
        <ResultView
          recording={selected}
          liveBody={liveBody}
          liveThinking={liveThinking}
          isStreaming={streamingId === selectedId}
          onCopyPrompt={(text) => {
            return copyBodyToClipboard(text);
          }}
          onRequestDelete={() => selected && setPendingDeleteId(selected.id)}
          onRetry={async () => {
            if (!selected || retrying) return;
            setRetrying(true);
            try {
              await ipc.retryRecording(selected.id);
              await refreshList();
            } finally {
              setRetrying(false);
            }
          }}
          retryDisabled={retrying}
        />
      ) : (
        <EmptyState />
      )}
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

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return target.isContentEditable || tag === 'input' || tag === 'textarea' || tag === 'select';
}

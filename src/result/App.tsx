import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { ipc, type HotkeyStatus, type Recording } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';
import { HistorySidebar } from './HistorySidebar';
import { ResultView } from './ResultView';
import { EmptyState } from './EmptyState';
import { Settings } from './Settings';

async function copyBodyToClipboard(text: string) {
  try {
    await writeText(text);
  } catch {
    try { await navigator.clipboard.writeText(text); } catch { /* best-effort */ }
  }
}

export function App() {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Live streaming buffer per-recording; cleared when stream ends.
  const liveRef = useRef<{ id: string; body: string } | null>(null);
  const [, force] = useState(0);
  const triggerRender = useCallback(() => force((n) => n + 1), []);

  // Tracks the most recent recording id we've auto-jumped to. The pill emits
  // a `recording` event on every elapsed-time tick; without this we'd reset
  // the user's manual sidebar selection on every tick.
  const autoSelectedRecRef = useRef<string | null>(null);

  const [streamingId, setStreamingId] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [keys, setKeys] = useState<{ openai: boolean; anthropic: boolean }>({ openai: false, anthropic: false });
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);

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
    void ipc.getApiKeyStatus().then(setKeys);
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

  // ↑/↓ navigate the sidebar.
  useGlobalKey(['ArrowDown', 'ArrowUp'], (e) => {
    if (showSettings) return;
    if (recordings.length === 0) return;
    const idx = recordings.findIndex((r) => r.id === selectedId);
    const next = e.key === 'ArrowDown'
      ? Math.min(recordings.length - 1, (idx < 0 ? 0 : idx + 1))
      : Math.max(0, (idx <= 0 ? 0 : idx - 1));
    setSelectedId(recordings[next].id);
    e.preventDefault();
  });

  // ⌘C copies the visible body when the markdown isn't focused.
  useGlobalKey('c', (e) => {
    if (!(e.metaKey || e.ctrlKey)) return;
    const sel = window.getSelection?.();
    if (sel && sel.toString().length > 0) return; // user selected text → default copy
    const visible = liveBody ?? selected?.body;
    if (visible) {
      e.preventDefault();
      void navigator.clipboard.writeText(visible);
    }
  });

  const selected = useMemo(
    () => recordings.find((r) => r.id === selectedId) ?? null,
    [recordings, selectedId],
  );

  const liveBody = liveRef.current && liveRef.current.id === selectedId
    ? liveRef.current.body
    : null;

  const needsKeys = !(keys.openai && keys.anthropic);
  const hasContent = !!selected;

  const showHotkeyWarning = hotkey !== null && !hotkey.installed;

  return (
    <div className="app">
      {showHotkeyWarning && (
        <div className="hotkey-banner" role="status">
          <span className="hotkey-banner__dot" aria-hidden />
          <span className="hotkey-banner__msg">
            <strong>Fn hotkey unavailable.</strong>{' '}
            {hotkey?.reason ?? 'Grant Peer Accessibility access in System Settings → Privacy & Security → Accessibility, then quit and reopen Peer.'}
          </span>
        </div>
      )}
      <HistorySidebar
        items={recordings}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onChanged={refreshList}
      />
      {hasContent ? (
        <ResultView
          recording={selected}
          liveBody={liveBody}
          isStreaming={streamingId === selectedId}
        />
      ) : (
        <EmptyState needsKeys={needsKeys} onOpenSettings={() => setShowSettings(true)} />
      )}
      <Settings
        open={showSettings}
        onClose={() => setShowSettings(false)}
        onSaved={() => { void ipc.getApiKeyStatus().then(setKeys); }}
      />
    </div>
  );
}


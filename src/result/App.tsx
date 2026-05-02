import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ipc, formatRelative, type HotkeyStatus, type Recording } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';
import { HistorySidebar } from './HistorySidebar';
import { ResultView } from './ResultView';
import { EmptyState } from './EmptyState';
import { Settings } from './Settings';

export function App() {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Live streaming buffer per-recording; cleared when stream ends.
  const liveRef = useRef<{ id: string; body: string } | null>(null);
  const [, force] = useState(0);
  const triggerRender = useCallback(() => force((n) => n + 1), []);

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
        e.kind === 'processing' || e.kind === 'done' || e.kind === 'error'
      ) {
        void refreshList();
      }
      if (e.kind === 'recording') {
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
        setSelectedId(c.id);
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
      <header className="app__header">
        <div className="app__brand" data-tauri-drag-region>
          <BrandMark />
          <span className="app__brandName">Hummingbird</span>
        </div>
        <div className="app__status" data-tauri-drag-region>
          {selected && <RecordingStatusLine recording={selected} />}
        </div>
      </header>
      {showHotkeyWarning && (
        <div className="hotkey-banner" role="status">
          <span className="hotkey-banner__dot" aria-hidden />
          <span className="hotkey-banner__msg">
            <strong>Fn hotkey unavailable.</strong>{' '}
            {hotkey?.reason ?? 'Grant Hummingbird Accessibility access in System Settings → Privacy & Security → Accessibility, then quit and reopen Hummingbird.'}
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

/** Halftone-orb brand mark. Coordinates match the pill logo and the macOS
 *  app icon so the brand reads consistently across surfaces. */
function BrandMark() {
  const dots: Array<[number, number, number]> = [
    [0, 0, 3.5],
    [0, -9, 3.0], [7.794, -4.5, 2.2], [7.794, 4.5, 3.0],
    [0, 9, 2.2], [-7.794, 4.5, 3.0], [-7.794, -4.5, 2.2],
    [6.123, -14.782, 2.4], [14.782, -6.123, 1.6], [14.782, 6.123, 2.4],
    [6.123, 14.782, 1.6], [-6.123, 14.782, 2.4], [-14.782, 6.123, 1.6],
    [-14.782, -6.123, 2.4], [-6.123, -14.782, 1.6],
  ];
  return (
    <svg
      viewBox="-22 -22 44 44"
      width="22"
      height="22"
      aria-hidden
      className="app__brandOrb"
    >
      {dots.map(([x, y, r], i) => (
        <circle key={i} cx={x} cy={y} r={r} fill="currentColor" />
      ))}
    </svg>
  );
}

function RecordingStatusLine({ recording }: { recording: Recording }) {
  const label =
    recording.status === 'recording' ? 'Recording'
    : recording.status === 'processing' ? 'Analyzing'
    : recording.status === 'stopped' ? 'Captured'
    : recording.status === 'failed' ? 'Failed'
    : recording.status === 'canceled' ? 'Canceled'
    : 'Done';
  const summary = recording.summary?.trim() || 'Untitled recording';
  return (
    <>
      <span
        className="app__statusDot"
        data-status={recording.status}
        aria-hidden
      />
      <span className="app__statusLabel">{label}</span>
      <span className="app__statusSep" aria-hidden>·</span>
      <span className="app__statusSummary" title={summary}>{summary}</span>
      <span className="app__statusMeta">{formatRelative(recording.createdAt)}</span>
    </>
  );
}

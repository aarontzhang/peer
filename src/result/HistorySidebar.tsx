import { useState } from 'react';
import { ipc, type Recording, formatDuration, formatRelative } from '@/lib/ipc';

type Props = {
  items: Recording[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onChanged: () => void;
};

export function HistorySidebar({ items, selectedId, onSelect, onChanged }: Props) {
  const [busy, setBusy] = useState(false);

  const onClearAll = async () => {
    if (busy || items.length === 0) return;
    const ok = window.confirm(
      `Delete all ${items.length} recording${items.length === 1 ? '' : 's'}? This can't be undone.`,
    );
    if (!ok) return;
    setBusy(true);
    try {
      await ipc.deleteAllRecordings();
      onChanged();
    } finally {
      setBusy(false);
    }
  };

  const onDelete = async (rec: Recording, e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy) return;
    setBusy(true);
    try {
      await ipc.deleteRecording(rec.id);
      onChanged();
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside className="sidebar">
      <div className="sidebar__brand" data-tauri-drag-region>
        <BrandMark />
        <span className="sidebar__brandName">Hummingbird</span>
      </div>
      <div className="sidebar__head">
        <span className="sidebar__title">History</span>
        <button
          type="button"
          className="sidebar__clear"
          onClick={onClearAll}
          disabled={busy || items.length === 0}
          aria-label="Delete all recordings"
          title="Delete all"
        >
          <TrashIcon />
        </button>
      </div>
      <div className="sidebar__list" role="listbox" aria-label="Recordings">
        {items.length === 0 && (
          <div style={{ padding: '12px 14px', color: 'var(--color-fg-dim)', fontSize: 12 }}>
            No recordings yet.
          </div>
        )}
        {items.map((rec) => {
          const title = rec.summary?.trim()
            || (rec.status === 'processing' ? 'Analyzing…'
                : rec.status === 'recording' ? 'Recording…'
                : rec.status === 'stopped' ? 'Captured (awaiting send)'
                : rec.status === 'canceled' ? 'Cancelled'
                : 'Untitled recording');
          return (
            <div
              key={rec.id}
              className="row-wrap"
              data-selected={rec.id === selectedId}
            >
              <button
                role="option"
                aria-selected={rec.id === selectedId}
                data-status={rec.status}
                className="row"
                onClick={() => onSelect(rec.id)}
              >
                <span className="row__pip" aria-hidden />
                <div className="row__body">
                  <div className="row__title">{title}</div>
                  <div className="row__meta">
                    {formatRelative(rec.createdAt)} · {formatDuration(rec.durationMs)}
                  </div>
                </div>
              </button>
              <button
                type="button"
                className="row__delete"
                onClick={(e) => onDelete(rec, e)}
                disabled={busy}
                aria-label={`Delete ${title}`}
                title="Delete"
              >
                <TrashIcon />
              </button>
            </div>
          );
        })}
      </div>
    </aside>
  );
}

/** Halftone-orb brand mark. Coordinates match the pill logo and the macOS
 *  app icon so the brand reads consistently across surfaces. */
function BrandMark() {
  const dots: Array<[number, number, number]> = [
    [0, 0, 3.5],
    // ring 1
    [0, -9, 3.0], [7.794, -4.5, 2.2], [7.794, 4.5, 3.0],
    [0, 9, 2.2], [-7.794, 4.5, 3.0], [-7.794, -4.5, 2.2],
    // ring 2
    [6.123, -14.782, 2.4], [14.782, -6.123, 1.6], [14.782, 6.123, 2.4],
    [6.123, 14.782, 1.6], [-6.123, 14.782, 2.4], [-14.782, 6.123, 1.6],
    [-14.782, -6.123, 2.4], [-6.123, -14.782, 1.6],
    // ring 3
    [3.519, -22.222, 1.8], [15.910, -15.910, 1.2], [22.222, -3.519, 1.8],
    [20.048, 10.215, 1.2], [10.215, 20.048, 1.8], [-3.519, 22.222, 1.2],
    [-15.910, 15.910, 1.8], [-22.222, 3.519, 1.2], [-20.048, -10.215, 1.8],
    [-10.215, -20.048, 1.2],
  ];
  return (
    <svg
      viewBox="-30 -30 60 60"
      aria-hidden
      className="sidebar__brandOrb"
    >
      {dots.map(([x, y, r], i) => (
        <circle key={i} cx={x} cy={y} r={r} fill="currentColor" />
      ))}
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path
        d="M3 4.5h10M6.5 4.5V3.2c0-.4.3-.7.7-.7h1.6c.4 0 .7.3.7.7v1.3M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8M7 7v4M9 7v4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

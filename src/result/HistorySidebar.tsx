import { useEffect, useRef, useState } from 'react';
import { ipc, type Recording, formatDuration, formatRelative } from '@/lib/ipc';
import { firstPlainTextLine } from '@/lib/plainText';

type Props = {
  items: Recording[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onChanged: () => void;
};

export function HistorySidebar({ items, selectedId, onSelect, onChanged }: Props) {
  const [busy, setBusy] = useState(false);

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
        <span className="sidebar__brandName">Peer</span>
      </div>
      <div className="sidebar__head">
        <span className="sidebar__title">History</span>
      </div>
      <div className="sidebar__list" role="listbox" aria-label="Recordings">
        {items.length === 0 && (
          <div style={{ padding: '12px 14px', color: 'var(--color-fg-dim)', fontSize: 12 }}>
            No recordings yet.
          </div>
        )}
        {items.map((rec) => {
          const title = firstPlainTextLine(rec.summary ?? rec.body ?? '')
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

/** Face + round glasses. Geometry matches the in-pill logo and the macOS
 *  app icon (src-tauri/icons/icon.svg) so the brand reads consistently
 *  across surfaces. The glasses translate as a unit on a random timer
 *  so the character idly glances around — see useRandomGaze. */
function BrandMark() {
  const maskId = 'sidebar-brand-head-mask';
  const [dx, dy] = useRandomGaze();
  const gazeTransform = `translate(${dx}px, ${dy}px)`;
  return (
    <svg
      viewBox="-50 -50 100 100"
      aria-hidden
      className="sidebar__brandOrb"
    >
      <defs>
        <mask id={maskId} maskUnits="userSpaceOnUse" x="-50" y="-50" width="100" height="100">
          <rect x="-50" y="-50" width="100" height="100" fill="white" />
          <g style={{ transform: gazeTransform }} fill="black">
            <circle cx="-15" cy="0" r="11.5" />
            <circle cx="15" cy="0" r="11.5" />
          </g>
        </mask>
      </defs>
      <g fill="none" stroke="currentColor" strokeLinecap="round">
        <circle cx="0" cy="0" r="37" strokeWidth="3" mask={`url(#${maskId})`} />
        <g strokeWidth="2.5" style={{ transform: gazeTransform }}>
          <circle cx="-15" cy="0" r="10" />
          <circle cx="15"  cy="0" r="10" />
          <line x1="-5" y1="0" x2="5" y2="0" />
        </g>
      </g>
    </svg>
  );
}

/* ─── Random gaze: glasses idly glance around ──────────────────────────── */
//
// Mirrors the cursor-gaze rig in src/pill/Pill.tsx, but the target is
// repicked from a random direction on a jittered timer instead of being
// derived from the mouse. Same lerp-each-frame damping so the saccade
// glides into place rather than snapping. Saturates short of GAZE_MAX
// so the lenses don't poke past the head outline.

const GAZE_MAX = 16;            // viewBox-unit cap on offset magnitude
const GAZE_LERP = 0.16;         // per-frame damping toward target
const GAZE_EPSILON = 0.05;      // snap to target once close enough
const HOLD_MIN_MS = 1400;       // min time to dwell on a target
const HOLD_MAX_MS = 3200;       // max dwell — randomised each pick
const REST_PROBABILITY = 0.25;  // chance the next pick is "look ahead"

function useRandomGaze(): [number, number] {
  const [gaze, setGaze] = useState<[number, number]>([0, 0]);
  const target = useRef<[number, number]>([0, 0]);
  const current = useRef<[number, number]>([0, 0]);

  // Repick the target on a jittered interval. A fraction of the time we
  // aim at the centre so the eyes occasionally rest instead of perpetually
  // ping-ponging around the rim.
  useEffect(() => {
    let timer: number | undefined;
    const pick = () => {
      if (Math.random() < REST_PROBABILITY) {
        target.current = [0, 0];
      } else {
        const angle = Math.random() * Math.PI * 2;
        // Bias toward the outer half of the range so motions read as
        // deliberate glances rather than tiny jitters.
        const r = GAZE_MAX * (0.55 + Math.random() * 0.45);
        target.current = [Math.cos(angle) * r, Math.sin(angle) * r];
      }
      const hold = HOLD_MIN_MS + Math.random() * (HOLD_MAX_MS - HOLD_MIN_MS);
      timer = window.setTimeout(pick, hold);
    };
    pick();
    return () => { if (timer !== undefined) window.clearTimeout(timer); };
  }, []);

  // Damped lerp toward target.
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

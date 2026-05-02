import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow, currentMonitor } from '@tauri-apps/api/window';
import { ipc, type PillEvent, formatDuration } from '@/lib/ipc';

export function Pill() {
  const [event, setEvent] = useState<PillEvent>({ kind: 'idle' });

  useEffect(() => {
    const unsub = ipc.onPillEvent((e) => setEvent(e));
    return () => { void unsub.then((fn) => fn()); };
  }, []);

  // Auto-fade processing→done back to idle after 1.4s.
  useEffect(() => {
    if (event.kind === 'done') {
      const t = window.setTimeout(() => setEvent({ kind: 'idle' }), 1400);
      return () => window.clearTimeout(t);
    }
  }, [event]);

  const state =
    event.kind === 'recording' ? 'recording'
    : event.kind === 'stopped' ? 'stopped'
    : event.kind === 'processing' ? 'processing'
    : event.kind === 'error' ? 'error'
    : event.kind === 'done' ? 'done'
    : 'idle';

  // Click on the logo or the dots: toggle recording. Drag on the dots moves
  // the pill instead — see useDragHandle below for the click-vs-drag split.
  // A clean Fn tap (handled in src-tauri/src/hotkey/fn_tap.rs) does the same
  // thing without having to aim at the pill.
  const toggleRecording = () => {
    if (state === 'recording') {
      void ipc.stopRecording();
    } else if (state === 'idle' || state === 'done' || state === 'error') {
      void ipc.startRecording();
    }
  };

  const onCancel = () => { void ipc.cancelRecording(); };
  const onSend = () => { void ipc.sendRecording(); };

  const elapsed = event.kind === 'recording' ? event.elapsedMs
    : event.kind === 'stopped' ? event.durationMs
    : 0;

  const dragHandle = useDragHandle(toggleRecording);

  return (
    <div className="pill" data-state={state}>
      {state === 'stopped' ? (
        <div className="pill__review">
          <button
            type="button"
            className="pill__review-btn pill__review-btn--cancel"
            onClick={onCancel}
            aria-label="Discard recording"
            title="Discard"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
              <path d="M3.5 3.5l9 9M12.5 3.5l-9 9" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
            </svg>
          </button>
          <button
            type="button"
            className="pill__review-btn pill__review-btn--send"
            onClick={onSend}
            aria-label="Send recording for processing"
            title={`Send (${formatDuration(elapsed)})`}
          >
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden>
              <path d="M3 8h9M8 4l4 4-4 4" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" fill="none" />
            </svg>
          </button>
        </div>
      ) : (
        <button
          type="button"
          className="pill__core"
          onClick={toggleRecording}
          aria-label={state === 'recording' ? 'Stop recording' : 'Start recording'}
          title={state === 'recording' ? formatDuration(elapsed) : 'Record'}
        >
          <TriangleLogo state={state} />
        </button>
      )}
      <div
        className="pill__handle"
        onPointerDown={dragHandle}
        aria-label="Drag pill"
        title="Drag"
      >
        <span className="pill__dot" />
        <span className="pill__dot" />
        <span className="pill__dot" />
      </div>
    </div>
  );
}

/* ─── Drag handle: clamps the pill inside the current monitor ──────────── */
//
// We manage the drag in JS rather than `data-tauri-drag-region` so we can
// (a) clamp against monitor bounds and (b) distinguish a clean click (fires
// `onClick`) from an actual drag (moves the window). Without the clamp the
// pill can be flung offscreen and become unrecoverable.

const DRAG_THRESHOLD_PX = 4;

function useDragHandle(onClick: () => void) {
  const dragging = useRef(false);

  return async (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    if (dragging.current) return;
    e.preventDefault();
    dragging.current = true;

    const win = getCurrentWindow();
    const monitor = await currentMonitor();
    const scale = monitor?.scaleFactor ?? 1;

    // Logical bounds of the current monitor and pill window.
    const monLogicalX = monitor ? monitor.position.x / scale : 0;
    const monLogicalY = monitor ? monitor.position.y / scale : 0;
    const monLogicalW = monitor ? monitor.size.width / scale : Number.POSITIVE_INFINITY;
    const monLogicalH = monitor ? monitor.size.height / scale : Number.POSITIVE_INFINITY;

    const winSize = await win.outerSize();
    const winW = winSize.width / scale;
    const winH = winSize.height / scale;

    const startWinPos = await win.outerPosition();
    const startWinX = startWinPos.x / scale;
    const startWinY = startWinPos.y / scale;

    const startMouseX = e.screenX;
    const startMouseY = e.screenY;

    const maxX = monLogicalX + monLogicalW - winW;
    const maxY = monLogicalY + monLogicalH - winH;
    const minX = monLogicalX;
    const minY = monLogicalY;

    let movedPastThreshold = false;
    let pending: { x: number; y: number } | null = null;
    let raf = 0;

    const flush = () => {
      raf = 0;
      if (!pending) return;
      const { x, y } = pending;
      pending = null;
      void ipc.movePill(x, y);
    };

    const onMove = (ev: PointerEvent) => {
      const dx = ev.screenX - startMouseX;
      const dy = ev.screenY - startMouseY;
      if (!movedPastThreshold && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      movedPastThreshold = true;
      const nx = Math.min(maxX, Math.max(minX, startWinX + dx));
      const ny = Math.min(maxY, Math.max(minY, startWinY + dy));
      pending = { x: nx, y: ny };
      if (!raf) raf = requestAnimationFrame(flush);
    };

    const onUp = () => {
      dragging.current = false;
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onUp);
      if (raf) {
        cancelAnimationFrame(raf);
        flush();
      }
      // No drag movement → treat the press as a click.
      if (!movedPastThreshold) onClick();
    };

    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
  };
}

/* ─── Halftone-orb logo ────────────────────────────────────────────────── */
//
// Concentric rings of dots in alternating sizes — the same pattern as the
// app icon, scaled down. While recording the whole orb spins; while
// processing the dots gently pulse. Pure SVG, no asset dependency.

function TriangleLogo({ state }: { state: string }) {
  return (
    <svg
      className="logo"
      viewBox="-30 -30 60 60"
      width="26"
      height="26"
      data-state={state}
      aria-hidden
    >
      <g className="logo__group">
        {ORB_DOTS.map((d, i) => (
          <circle key={i} cx={d.x} cy={d.y} r={d.r} />
        ))}
      </g>
    </svg>
  );
}

/** Halftone-orb dot positions. Center + three rings. Sizes alternate
 *  within each ring so the pattern reads as varied rather than gridded.
 *  Coordinates match icons/icon.svg so the pill and the app icon are
 *  visibly the same mark. */
const ORB_DOTS: Array<{ x: number; y: number; r: number }> = [
  { x: 0,        y: 0,        r: 3.5 },

  // ring 1 (radius 9, 6 dots)
  { x: 0,        y: -9,       r: 3.0 },
  { x: 7.794,    y: -4.500,   r: 2.2 },
  { x: 7.794,    y: 4.500,    r: 3.0 },
  { x: 0,        y: 9,        r: 2.2 },
  { x: -7.794,   y: 4.500,    r: 3.0 },
  { x: -7.794,   y: -4.500,   r: 2.2 },

  // ring 2 (radius 16, 8 dots)
  { x: 6.123,    y: -14.782,  r: 2.4 },
  { x: 14.782,   y: -6.123,   r: 1.6 },
  { x: 14.782,   y: 6.123,    r: 2.4 },
  { x: 6.123,    y: 14.782,   r: 1.6 },
  { x: -6.123,   y: 14.782,   r: 2.4 },
  { x: -14.782,  y: 6.123,    r: 1.6 },
  { x: -14.782,  y: -6.123,   r: 2.4 },
  { x: -6.123,   y: -14.782,  r: 1.6 },

  // ring 3 (radius 22.5, 10 dots)
  { x: 3.519,    y: -22.222,  r: 1.8 },
  { x: 15.910,   y: -15.910,  r: 1.2 },
  { x: 22.222,   y: -3.519,   r: 1.8 },
  { x: 20.048,   y: 10.215,   r: 1.2 },
  { x: 10.215,   y: 20.048,   r: 1.8 },
  { x: -3.519,   y: 22.222,   r: 1.2 },
  { x: -15.910,  y: 15.910,   r: 1.8 },
  { x: -22.222,  y: 3.519,    r: 1.2 },
  { x: -20.048,  y: -10.215,  r: 1.8 },
  { x: -10.215,  y: -20.048,  r: 1.2 },
];

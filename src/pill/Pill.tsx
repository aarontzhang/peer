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
  const gaze = useGazeDirection(state === 'recording');

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
          <GlassesLogo state={state} direction={gaze} />
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

/* ─── Face + glasses logo ──────────────────────────────────────────────── */
//
// A round head with a pair of round glasses inside. The glasses translate
// as a unit to point the gaze in one of six directions — this is what
// gives the pill its "looking at you" character while recording. The
// straight-ahead variant is the same mark used for the macOS app icon
// (src-tauri/icons/icon.svg), so pill and dock read as the same face.

type GazeDirection =
  | 'straight'
  | 'up-left'
  | 'up-right'
  | 'down'
  | 'down-left'
  | 'down-right';

// Glasses-center offset (in viewBox units) per direction. The viewBox is
// -50..50, so these are roughly percentage-of-radius offsets. Diagonals
// push hard enough that the outer lens pokes past the head circle — the
// mask below clips the head stroke inside the lens so there's no double
// line where they cross.
const GAZE_OFFSETS: Record<GazeDirection, [number, number]> = {
  'straight':   [  0,   0],
  'up-left':    [-15, -13],
  'up-right':   [ 15, -13],
  'down':       [  0,  14],
  'down-left':  [-13,  12],
  'down-right': [ 13,  12],
};

function GlassesLogo({ state, direction }: { state: string; direction: GazeDirection }) {
  const [dx, dy] = GAZE_OFFSETS[direction];
  const maskId = 'pill-head-mask';
  const gazeTransform = `translate(${dx}px, ${dy}px)`;
  return (
    <svg
      className="logo"
      viewBox="-50 -50 100 100"
      width="26"
      height="26"
      data-state={state}
      aria-hidden
    >
      <defs>
        {/* Punch the lens discs out of the head's stroke so it never shows
            through where the glasses cross or extend past the face. Hole
            radius is lens radius + lens stroke so the head stroke is fully
            erased under the lens stroke too. */}
        <mask id={maskId} maskUnits="userSpaceOnUse" x="-50" y="-50" width="100" height="100">
          <rect x="-50" y="-50" width="100" height="100" fill="white" />
          <g style={{ transform: gazeTransform }} fill="black">
            <circle cx="-15" cy="0" r="11.5" />
            <circle cx="15"  cy="0" r="11.5" />
          </g>
        </mask>
      </defs>
      <g className="logo__group" fill="none" strokeLinecap="round">
        {/* head */}
        <circle
          className="logo__head"
          cx="0"
          cy="0"
          r="37"
          strokeWidth="3"
          mask={`url(#${maskId})`}
        />
        {/* glasses — translated so the character "looks" toward dx,dy */}
        <g
          className="logo__glasses"
          strokeWidth="2.5"
          style={{ transform: gazeTransform }}
        >
          <circle cx="-15" cy="0" r="10" />
          <circle cx="15"  cy="0" r="10" />
          <line x1="-5" y1="0" x2="5" y2="0" />
        </g>
      </g>
    </svg>
  );
}

/* ─── Gaze direction: which way to look so we face the screen center ───── */
//
// When recording starts, we figure out where on the monitor the pill is
// sitting and pick a gaze direction that points back toward the middle of
// the screen. Top-left pill → looks down-right; bottom-right → up-left;
// roughly centered → straight. Recomputes whenever recording (re)starts.

function useGazeDirection(active: boolean): GazeDirection {
  const [dir, setDir] = useState<GazeDirection>('straight');
  useEffect(() => {
    if (!active) { setDir('straight'); return; }
    let cancelled = false;
    void (async () => {
      try {
        const win = getCurrentWindow();
        const monitor = await currentMonitor();
        if (!monitor || cancelled) return;
        const scale = monitor.scaleFactor || 1;
        const winPos = await win.outerPosition();
        const winSize = await win.outerSize();
        const px = (winPos.x + winSize.width / 2) / scale;
        const py = (winPos.y + winSize.height / 2) / scale;
        const monW = monitor.size.width / scale;
        const monH = monitor.size.height / scale;
        const mx = monitor.position.x / scale + monW / 2;
        const my = monitor.position.y / scale + monH / 2;
        const dx = mx - px;   // + → screen center is to the right of pill
        const dy = my - py;   // + → screen center is below the pill
        // Dead zones: if the pill is roughly aligned with the center on
        // an axis, don't bias the gaze along that axis.
        const hThresh = monW * 0.12;
        const vThresh = monH * 0.12;
        if (cancelled) return;
        let next: GazeDirection;
        if (dy > vThresh) {
          // Pill is in the top half → look down.
          next = dx > hThresh ? 'down-right' : dx < -hThresh ? 'down-left' : 'down';
        } else if (dy < -vThresh) {
          // Pill is in the bottom half → look up. We have no plain "up"
          // icon, so when the pill is bottom-center we still pick a
          // diagonal toward whichever side dx leans (or up-right by
          // default if perfectly centered).
          next = dx >= 0 ? 'up-right' : 'up-left';
          if (dx > hThresh) next = 'up-right';
          else if (dx < -hThresh) next = 'up-left';
        } else {
          next = 'straight';
        }
        setDir(next);
      } catch {
        // Tauri APIs occasionally throw during teardown — fall back to straight.
      }
    })();
    return () => { cancelled = true; };
  }, [active]);
  return dir;
}

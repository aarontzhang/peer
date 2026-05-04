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
  const gaze = useCursorGaze(state === 'recording');

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
          <GlassesLogo state={state} dx={gaze[0]} dy={gaze[1]} />
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
// as a unit to follow the cursor while recording — see useCursorGaze.
// The straight-ahead variant (dx=dy=0) is the same mark used for the
// macOS app icon (src-tauri/icons/icon.svg), so pill and dock read as
// the same face.

function GlassesLogo({ state, dx, dy }: { state: string; dx: number; dy: number }) {
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

/* ─── Cursor gaze: glasses follow the mouse pointer ────────────────────── */
//
// While recording, the pill polls the global cursor position each animation
// frame and aims the glasses at it. The raw vector (cursor → pill center)
// is normalised, scaled to viewBox units, then lerped each frame so the
// motion damps smoothly instead of snapping. Beyond ~RANGE pixels the
// gaze saturates — the lens is already poking past the head circle and
// further pulling would just look weird.
//
// We pull cursor position from Rust (CGEventSource on macOS) since the
// pill window only sees mousemove events while the cursor is over it.

const GAZE_MAX = 20;          // viewBox-unit cap on offset magnitude
const GAZE_RANGE_PX = 240;    // distance at which gaze saturates
const GAZE_LERP = 0.34;       // per-frame damping toward target
const GAZE_EPSILON = 0.05;    // stop animating when target is reached

function useCursorGaze(active: boolean): [number, number] {
  const [gaze, setGaze] = useState<[number, number]>([0, 0]);
  const target = useRef<[number, number]>([0, 0]);
  const current = useRef<[number, number]>([0, 0]);

  // Poll the cursor position on rAF and update the gaze target. Window
  // position/size are fetched async, so we cache and refresh them on a
  // slower cadence — they only change when the user drags the pill.
  useEffect(() => {
    if (!active) {
      target.current = [0, 0];
      return;
    }
    let cancelled = false;
    let raf = 0;
    let winCenterLogical: [number, number] | null = null;
    let winRefreshDue = 0;
    let nextPollDue = 0;
    let scale = 1;

    const refreshWindow = async () => {
      const win = getCurrentWindow();
      const [pos, size, monitor] = await Promise.all([
        win.outerPosition(),
        win.outerSize(),
        currentMonitor(),
      ]);
      scale = monitor?.scaleFactor || 1;
      winCenterLogical = [
        (pos.x + size.width / 2) / scale,
        (pos.y + size.height / 2) / scale,
      ];
    };
    void refreshWindow();

    const tick = async (now: number) => {
      if (cancelled) return;
      if (now < nextPollDue) {
        raf = requestAnimationFrame(tick);
        return;
      }
      nextPollDue = now + 16; // ~60Hz cursor poll; lerp fills in the rest
      if (now >= winRefreshDue) {
        winRefreshDue = now + 250;
        try { await refreshWindow(); } catch { /* ignore */ }
      }
      try {
        const [cx, cy] = await ipc.cursorPosition();
        if (cancelled || !winCenterLogical) {
          raf = requestAnimationFrame(tick);
          return;
        }
        const dxPx = cx - winCenterLogical[0];
        const dyPx = cy - winCenterLogical[1];
        const dist = Math.hypot(dxPx, dyPx);
        if (dist < 1) {
          target.current = [0, 0];
        } else {
          const norm = Math.min(dist / GAZE_RANGE_PX, 1);
          const k = (GAZE_MAX * norm) / dist;
          target.current = [dxPx * k, dyPx * k];
        }
      } catch {
        // IPC may fail momentarily during teardown — keep the previous target.
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);

    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
    };
  }, [active]);

  // Damped lerp toward the target. Runs whenever the target diverges from
  // the rendered gaze, then idles once the spring settles.
  useEffect(() => {
    let raf = 0;
    let stopped = false;
    const step = () => {
      const [tx, ty] = target.current;
      const [cx, cy] = current.current;
      const nx = cx + (tx - cx) * GAZE_LERP;
      const ny = cy + (ty - cy) * GAZE_LERP;
      current.current = [nx, ny];
      setGaze([
        Math.abs(nx) < GAZE_EPSILON && tx === 0 ? 0 : nx,
        Math.abs(ny) < GAZE_EPSILON && ty === 0 ? 0 : ny,
      ]);
      if (!stopped) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => { stopped = true; cancelAnimationFrame(raf); };
  }, []);

  return gaze;
}

import { useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { getCurrentWindow, currentMonitor } from '@tauri-apps/api/window';
import { ipc, type PillEvent, formatDuration } from '@/lib/ipc';

export function Pill() {
  const [event, setEvent] = useState<PillEvent>({ kind: 'idle' });
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const unsub = ipc.onPillEvent((e) => setEvent(e));
    return () => { void unsub.then((fn) => fn()); };
  }, []);

  // Auto-fade copied/done feedback back to idle after a short confirmation.
  useEffect(() => {
    if (event.kind === 'done') {
      const t = window.setTimeout(() => setEvent({ kind: 'idle' }), 3000);
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

  // Click on the logo: toggle recording. Holding anywhere on the pill and
  // dragging moves the window instead — see useDragOnHold for the
  // click-vs-drag split. The configured recording keybind does the same
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

  useEffect(() => {
    if (event.kind !== 'stopped') return;

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.repeat || isEditableTarget(e.target)) return;
      if (e.key === 'Enter') {
        e.preventDefault();
        e.stopPropagation();
        void ipc.sendRecording();
        return;
      }
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        e.stopPropagation();
        void ipc.cancelRecording();
      }
    };

    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [event.kind]);

  const elapsed = event.kind === 'recording' ? event.elapsedMs
    : event.kind === 'stopped' ? event.durationMs
    : 0;

  const onPillPointerDown = useDragOnHold(rootRef);
  const gaze = useCursorGaze(state === 'recording');

  return (
    <div className="pill" data-state={state} ref={rootRef} onPointerDown={onPillPointerDown}>
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
      <div className="pill__handle" aria-hidden>
        <span className="pill__dot" />
        <span className="pill__dot" />
        <span className="pill__dot" />
      </div>
    </div>
  );
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return target.isContentEditable || tag === 'input' || tag === 'textarea' || tag === 'select';
}

/* ─── Drag-on-hold: clamps the pill near the current monitor ────────────── */
//
// We manage the drag in JS rather than `data-tauri-drag-region` so we can
// (a) clamp against monitor bounds and (b) distinguish a clean click on the
// inner buttons from an actual drag. Pointerdown anywhere on the pill starts
// the drag; if the pointer never moves past DRAG_THRESHOLD_PX, the underlying
// button receives a normal click. Otherwise we swallow the trailing click so
// dragging from the record/cancel/send buttons doesn't also trigger them.
//
// The pill *window* is larger than the visible pill (transparent padding for
// shadow/breathing room), so we let the window bleed past the monitor edge
// by `pillInset − EDGE_GAP_PX` on each side. That way the visible pill can
// sit a small visual gap from the screen edge instead of stopping wherever
// the invisible window edge happens to land.

const DRAG_THRESHOLD_PX = 4;
const EDGE_GAP_PX = 4;

function useDragOnHold(rootRef: RefObject<HTMLDivElement | null>) {
  const dragging = useRef(false);
  const suppressClick = useRef(false);

  useEffect(() => {
    const onClickCapture = (e: MouseEvent) => {
      if (!suppressClick.current) return;
      suppressClick.current = false;
      e.preventDefault();
      e.stopPropagation();
      e.stopImmediatePropagation();
    };
    window.addEventListener('click', onClickCapture, true);
    return () => window.removeEventListener('click', onClickCapture, true);
  }, []);

  return async (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    if (dragging.current) return;
    dragging.current = true;

    const win = getCurrentWindow();
    const monitor = await currentMonitor();
    const scale = monitor?.scaleFactor ?? 1;

    const monLogicalX = monitor ? monitor.position.x / scale : 0;
    const monLogicalY = monitor ? monitor.position.y / scale : 0;
    const monLogicalW = monitor ? monitor.size.width / scale : Number.POSITIVE_INFINITY;
    const monLogicalH = monitor ? monitor.size.height / scale : Number.POSITIVE_INFINITY;

    const winSize = await win.outerSize();
    const winW = winSize.width / scale;
    const winH = winSize.height / scale;

    // Visible-pill inset within the (larger) window — read from the DOM so we
    // don't hardcode magic numbers tied to padding/sizing in CSS.
    let bleedL = 0, bleedT = 0, bleedR = 0, bleedB = 0;
    const node = rootRef.current;
    if (node) {
      const rect = node.getBoundingClientRect();
      bleedL = Math.max(0, rect.left - EDGE_GAP_PX);
      bleedT = Math.max(0, rect.top - EDGE_GAP_PX);
      bleedR = Math.max(0, window.innerWidth - rect.right - EDGE_GAP_PX);
      bleedB = Math.max(0, window.innerHeight - rect.bottom - EDGE_GAP_PX);
    }

    const startWinPos = await win.outerPosition();
    const startWinX = startWinPos.x / scale;
    const startWinY = startWinPos.y / scale;

    const startMouseX = e.screenX;
    const startMouseY = e.screenY;

    const minX = monLogicalX - bleedL;
    const minY = monLogicalY - bleedT;
    const maxX = monLogicalX + monLogicalW - winW + bleedR;
    const maxY = monLogicalY + monLogicalH - winH + bleedB;

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
      if (movedPastThreshold) {
        // Eat the trailing click so a drag that started on a button doesn't
        // also fire its onClick. Reset shortly in case no click ever comes.
        suppressClick.current = true;
        window.setTimeout(() => { suppressClick.current = false; }, 60);
      }
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
            <circle cx="-15" cy="0" r="12.5" />
            <circle cx="15"  cy="0" r="12.5" />
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
          strokeWidth="5"
          mask={`url(#${maskId})`}
        />
        {/* glasses — translated so the character "looks" toward dx,dy */}
        <g
          className="logo__glasses"
          strokeWidth="4"
          style={{ transform: gazeTransform }}
        >
          <circle className="logo__lens-fill" cx="-15" cy="0" r="8.75" />
          <circle className="logo__lens-fill" cx="15" cy="0" r="8.75" />
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

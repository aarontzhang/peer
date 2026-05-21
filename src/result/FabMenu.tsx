import { useCallback, useEffect, useRef, useState } from 'react';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { ipc } from '@/lib/ipc';
import { useGlobalKey } from '@/lib/keys';

type Props = {
  /** True when a recording or pipeline is in flight — the upload action is
   *  disabled with a tooltip so the user doesn't fire one off and get a Rust
   *  error toast for their trouble. */
  busy: boolean;
  /** Switch the feed back to History so the brand-new processing card is
   *  visible the moment the IPC returns. */
  onUploadStarted: () => void;
  /** Surface the Rust error to the user (toast, banner — caller decides). */
  onUploadError: (message: string) => void;
};

const VIDEO_EXTENSIONS = ['mp4', 'mov', 'm4v', 'webm', 'mkv', 'avi'];

export function FabMenu({ busy, onUploadStarted, onUploadError }: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [staging, setStaging] = useState(false);
  const containerRef = useRef<HTMLDivElement | null>(null);

  const closeMenu = useCallback(() => setMenuOpen(false), []);

  useGlobalKey('Escape', (e) => {
    if (menuOpen) {
      e.preventDefault();
      closeMenu();
    }
  });

  // Click outside closes the menu.
  useEffect(() => {
    if (!menuOpen) return;
    const onDocClick = (e: MouseEvent) => {
      if (!containerRef.current) return;
      if (e.target instanceof Node && containerRef.current.contains(e.target)) {
        return;
      }
      closeMenu();
    };
    document.addEventListener('mousedown', onDocClick);
    return () => document.removeEventListener('mousedown', onDocClick);
  }, [menuOpen, closeMenu]);

  const handleUpload = useCallback(async () => {
    if (staging || busy) return;
    closeMenu();
    setStaging(true);
    let picked: string | null = null;
    try {
      const result = await openFileDialog({
        title: 'Upload a video to Peer',
        multiple: false,
        directory: false,
        filters: [{ name: 'Video', extensions: VIDEO_EXTENSIONS }],
      });
      picked = typeof result === 'string' ? result : null;
      if (!picked) return;
      onUploadStarted();
      await ipc.uploadRecording(picked);
    } catch (err) {
      onUploadError(err instanceof Error ? err.message : String(err));
    } finally {
      setStaging(false);
    }
  }, [staging, busy, closeMenu, onUploadStarted, onUploadError]);

  // ⌘O / Ctrl-O opens the picker directly without going through the menu.
  // The handler-ref pattern keeps a single window listener while still
  // using the latest `handleUpload` (no stale closures, no eslint disable).
  const handleUploadRef = useRef(handleUpload);
  useEffect(() => {
    handleUploadRef.current = handleUpload;
  }, [handleUpload]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'o') {
        e.preventDefault();
        void handleUploadRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div ref={containerRef} className="fab-stack">
      {menuOpen && (
        <div className="fab-menu" role="menu" aria-label="Add to Peer">
          <button
            type="button"
            role="menuitem"
            className="fab-menu__item"
            onClick={handleUpload}
            disabled={busy || staging}
            title={busy ? 'Wait for the current recording to finish' : undefined}
          >
            <span className="fab-menu__icon" aria-hidden>
              <UploadIcon />
            </span>
            <span className="fab-menu__label">
              <span className="fab-menu__title">Upload video</span>
              <span className="fab-menu__sub">
                {busy
                  ? 'Wait for current recording'
                  : 'Pick a video from your Mac · ⌘O'}
              </span>
            </span>
          </button>
        </div>
      )}
      <button
        type="button"
        className={`fab${menuOpen ? ' fab--open' : ''}${staging ? ' fab--busy' : ''}`}
        onClick={() => setMenuOpen((open) => !open)}
        aria-expanded={menuOpen}
        aria-haspopup="menu"
        aria-label={menuOpen ? 'Close add menu' : 'Add to Peer'}
        disabled={staging}
      >
        <span className="fab__icon" aria-hidden>
          {staging ? <Spinner /> : <PlusIcon />}
        </span>
      </button>
    </div>
  );
}

function PlusIcon() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden focusable="false">
      <path
        d="M12 5v14M5 12h14"
        stroke="currentColor"
        strokeWidth="2.4"
        strokeLinecap="round"
      />
    </svg>
  );
}

function UploadIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden focusable="false">
      <path
        d="M12 16V4m0 0l-5 5m5-5l5 5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M4 16v2.5A1.5 1.5 0 0 0 5.5 20h13a1.5 1.5 0 0 0 1.5-1.5V16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
      />
    </svg>
  );
}

function Spinner() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden focusable="false">
      <circle
        cx="12"
        cy="12"
        r="9"
        fill="none"
        stroke="currentColor"
        strokeOpacity="0.25"
        strokeWidth="2.4"
      />
      <path
        d="M12 3a9 9 0 0 1 9 9"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.4"
        strokeLinecap="round"
      >
        <animateTransform
          attributeName="transform"
          type="rotate"
          from="0 12 12"
          to="360 12 12"
          dur="0.9s"
          repeatCount="indefinite"
        />
      </path>
    </svg>
  );
}

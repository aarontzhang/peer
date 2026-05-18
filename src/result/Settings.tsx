import { useEffect, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type DownloadEvent } from '@tauri-apps/plugin-updater';
import {
  ipc,
  type AccountStatus,
  type AuthChangedPayload,
  type HotkeyStatus,
  type PermissionMode,
  type RecordingKeybind,
} from '@/lib/ipc';

type Props = {
  open: boolean;
  onClose: () => void;
};

const DEFAULT_KEYBIND: RecordingKeybind = { kind: 'rightOption' };

type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'none' }
  | { kind: 'downloading'; version: string; downloaded: number; total: number | null }
  | { kind: 'installing'; version: string }
  | { kind: 'relaunching'; version: string }
  | { kind: 'error'; message: string };

export function Settings({ open, onClose }: Props) {
  const [account, setAccount] = useState<AccountStatus | null>(null);
  const [pendingSignIn, setPendingSignIn] = useState(false);
  const [signInError, setSignInError] = useState<string | null>(null);
  const [noAccount, setNoAccount] = useState(false);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [keybind, setKeybind] = useState<RecordingKeybind>(DEFAULT_KEYBIND);
  const [initialKeybind, setInitialKeybind] = useState<RecordingKeybind>(DEFAULT_KEYBIND);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [mode, setMode] = useState<PermissionMode>('ask');
  const [initialMode, setInitialMode] = useState<PermissionMode>('ask');
  const [updateState, setUpdateState] = useState<UpdateState>({ kind: 'idle' });

  useEffect(() => {
    if (open) {
      void ipc.getSession().then(setAccount);
      void ipc.getHotkeyStatus().then((next) => {
        setHotkey(next);
        setKeybind(next.keybind);
        setInitialKeybind(next.keybind);
      });
      void ipc.getPermissionMode().then((next) => {
        setMode(next);
        setInitialMode(next);
      });
      setPendingSignIn(false);
      setSignInError(null);
      setNoAccount(false);
      setCapturing(false);
      setCaptureError(null);
      setUpdateState({ kind: 'idle' });
    }
  }, [open]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    ipc
      .onAuthChanged(async (payload: AuthChangedPayload) => {
        setAccount(await ipc.getSession());
        setPendingSignIn(false);
        if (payload.reason === 'no_account') {
          setNoAccount(true);
          setSignInError(null);
        } else if (payload.error) {
          setSignInError(payload.error);
          setNoAccount(false);
        } else if (payload.signedIn) {
          setSignInError(null);
          setNoAccount(false);
        }
      })
      .then((u) => {
        unlisten = u;
      });
    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!pendingSignIn) return;

    const timeout = window.setTimeout(() => {
      setPendingSignIn(false);
      setSignInError('Sign-in timed out — open Settings and try again.');
    }, 10 * 60 * 1000);

    return () => window.clearTimeout(timeout);
  }, [pendingSignIn]);

  useEffect(() => {
    if (!capturing) return;

    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      // Wait until a non-modifier key is pressed (chord) or a solo modifier
      // is released (handled in onKeyUp).
      if (isModifierCode(e.code)) return;

      const next = chordFromEvent(e);
      if (!next) {
        setCaptureError('That key combination is not supported. Try another.');
        return;
      }
      if (next.kind === 'chord' && next.mods.length === 0) {
        setCaptureError(
          'A chord needs at least one modifier (⌘, ⌃, ⌥, or ⇧). A bare key would steal it from every app.',
        );
        return;
      }
      setKeybind(next);
      setCaptureError(null);
      setCapturing(false);
    };

    const onKeyUp = (e: KeyboardEvent) => {
      // Solo Right-Option tap: AltRight released with no other modifiers.
      if (e.code === 'AltRight' && !e.metaKey && !e.ctrlKey && !e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
        setKeybind({ kind: 'rightOption' });
        setCaptureError(null);
        setCapturing(false);
      }
    };

    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
    };
  }, [capturing]);

  if (!open) return null;

  const onSave = async () => {
    setSaving(true);
    try {
      if (!keybindEqual(keybind, initialKeybind)) {
        const next = await ipc.setRecordingKeybind(keybind);
        setHotkey(next);
        setInitialKeybind(next.keybind);
        setKeybind(next.keybind);
      }
      if (mode !== initialMode) {
        const next = await ipc.setPermissionMode(mode);
        setMode(next);
        setInitialMode(next);
      }
      onClose();
    } finally {
      setSaving(false);
    }
  };

  const onLogin = async () => {
    setSignInError(null);
    setNoAccount(false);
    setPendingSignIn(true);
    try {
      await ipc.startGoogleSignIn();
    } catch (err) {
      setPendingSignIn(false);
      setSignInError(err instanceof Error ? err.message : String(err));
    }
  };

  const onSignOut = async () => {
    await ipc.signOut();
    setAccount(await ipc.getSession());
  };

  const onCheckForUpdates = async () => {
    setUpdateState({ kind: 'checking' });
    try {
      const update = await check();
      if (!update) {
        setUpdateState({ kind: 'none' });
        return;
      }

      let downloaded = 0;
      let total: number | null = null;
      setUpdateState({ kind: 'downloading', version: update.version, downloaded, total });
      await update.downloadAndInstall((event: DownloadEvent) => {
        if (event.event === 'Started') {
          total = event.data.contentLength ?? null;
          setUpdateState({ kind: 'downloading', version: update.version, downloaded, total });
        } else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength;
          setUpdateState({ kind: 'downloading', version: update.version, downloaded, total });
        } else {
          setUpdateState({ kind: 'installing', version: update.version });
        }
      });
      setUpdateState({ kind: 'relaunching', version: update.version });
      await relaunch();
    } catch (err) {
      setUpdateState({ kind: 'error', message: err instanceof Error ? err.message : String(err) });
    }
  };

  const changed = !keybindEqual(keybind, initialKeybind) || mode !== initialMode;
  const showHotkeyProblem =
    hotkey && keybindEqual(hotkey.keybind, keybind) && !hotkey.installed;
  const updateBusy =
    updateState.kind === 'checking' ||
    updateState.kind === 'downloading' ||
    updateState.kind === 'installing' ||
    updateState.kind === 'relaunching';

  return (
    <div className="settings" role="dialog" aria-modal="true" onClick={onClose}>
      <div className="settings__panel" onClick={(e) => e.stopPropagation()}>
        <h2 className="settings__title">Settings</h2>

        <section className="settings__section">
          <div className="settings__sectionHead">
            <h3 className="settings__sectionTitle">Account</h3>
          </div>
          {account?.signedIn ? (
            <div className="settings__row">
              <span className="settings__email">
                <span className="settings__emailLabel">Email:</span>{' '}
                {account.email ?? 'Signed in'}
              </span>
              <button className="btn btn--neutral" type="button" onClick={onSignOut}>
                Sign out
              </button>
            </div>
          ) : noAccount ? (
            <>
              <p className="settings__hint">
                Hey, you don't have an account yet. Create one to start using Peer.
              </p>
              <div className="settings__row">
                <button
                  className="btn btn--neutral"
                  type="button"
                  disabled
                  title="Sign-up isn't available yet"
                >
                  Creating account is disabled
                </button>
                <button
                  className="btn btn--neutral"
                  type="button"
                  onClick={() => {
                    setNoAccount(false);
                    setSignInError(null);
                  }}
                >
                  Use a different account
                </button>
              </div>
            </>
          ) : (
            <div className="settings__row">
              <button
                className="btn btn--neutral"
                type="button"
                onClick={onLogin}
                disabled={pendingSignIn}
              >
                Sign in
              </button>
            </div>
          )}
          {pendingSignIn && !account?.signedIn && !noAccount && (
            <p className="settings__hint">Waiting for browser sign-in…</p>
          )}
          {signInError && !noAccount && <p className="settings__error">{signInError}</p>}
        </section>

        <section className="settings__section">
          <div className="settings__sectionHead">
            <h3 className="settings__sectionTitle">Recording shortcut</h3>
          </div>
          <div className="settings__shortcut">
            <button
              className={`shortcut-display${capturing ? ' shortcut-display--capturing' : ''}`}
              type="button"
              onClick={() => {
                setCapturing(true);
                setCaptureError(null);
              }}
              aria-label="Set recording shortcut"
            >
              {capturing ? (
                <span className="shortcut-display__hint">Press any key…</span>
              ) : (
                <KeybindKeys keybind={keybind} />
              )}
            </button>
            {capturing ? (
              <button
                className="btn btn--neutral"
                type="button"
                onClick={() => setCapturing(false)}
              >
                Cancel
              </button>
            ) : (
              <button
                className="btn btn--neutral"
                type="button"
                onClick={() => {
                  setCapturing(true);
                  setCaptureError(null);
                }}
              >
                Change
              </button>
            )}
          </div>
          <p className="settings__hint">
            {keybind.kind === 'chord'
              ? 'Press your shortcut anywhere to start or stop a recording.'
              : `Tap ${keybindLabel(keybind)} to start or stop recording. The pill click still works.`}
          </p>
          {captureError && <p className="settings__error">{captureError}</p>}
          {showHotkeyProblem && hotkey?.reason && (
            <p className="settings__error">{hotkey.reason}</p>
          )}
        </section>

        <section className="settings__section">
          <div className="settings__sectionHead">
            <h3 className="settings__sectionTitle">Mode</h3>
          </div>
          <div className="settings__segmented" role="radiogroup" aria-label="Permission mode">
            <button
              type="button"
              role="radio"
              aria-checked={mode === 'ask'}
              className={`settings__segmented__item${mode === 'ask' ? ' settings__segmented__item--active' : ''}`}
              onClick={() => setMode('ask')}
            >
              Ask permission
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={mode === 'bypass'}
              className={`settings__segmented__item${mode === 'bypass' ? ' settings__segmented__item--active' : ''}`}
              onClick={() => setMode('bypass')}
            >
              Allow everything
            </button>
          </div>
          <p className="settings__hint">
            {mode === 'ask'
              ? 'The generated prompt tells the agent to check in with you before destructive or critical steps.'
              : 'The generated prompt tells the agent to run end-to-end without asking.'}
          </p>
        </section>

        <section className="settings__section">
          <div className="settings__sectionHead">
            <h3 className="settings__sectionTitle">Updates</h3>
          </div>
          <div className="settings__row">
            <button
              className="btn btn--neutral"
              type="button"
              onClick={onCheckForUpdates}
              disabled={updateBusy}
            >
              {updateBusy ? updateButtonLabel(updateState) : 'Check for updates'}
            </button>
          </div>
          <UpdateStatus state={updateState} />
        </section>

        <div className="settings__actions">
          <button className="btn btn--neutral btn--neutralDark" onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button
            className="btn btn--neutral btn--neutralLight"
            onClick={onSave}
            disabled={saving}
          >
            {saving ? 'Saving…' : changed ? 'Save' : 'Done'}
          </button>
        </div>
      </div>
    </div>
  );
}

function UpdateStatus({ state }: { state: UpdateState }) {
  if (state.kind === 'idle') return null;
  if (state.kind === 'checking') {
    return <p className="settings__hint">Checking for updates...</p>;
  }
  if (state.kind === 'none') {
    return <p className="settings__hint">Peer is up to date.</p>;
  }
  if (state.kind === 'downloading') {
    const label = formatUpdateProgress(state.downloaded, state.total);
    return (
      <p className="settings__hint">
        Downloading Peer {state.version}
        {label ? ` (${label})` : ''}
      </p>
    );
  }
  if (state.kind === 'installing') {
    return <p className="settings__hint">Installing Peer {state.version}...</p>;
  }
  if (state.kind === 'relaunching') {
    return <p className="settings__hint">Relaunching Peer {state.version}...</p>;
  }
  return <p className="settings__error">{state.message}</p>;
}

function updateButtonLabel(state: UpdateState): string {
  if (state.kind === 'checking') return 'Checking...';
  if (state.kind === 'downloading') return 'Downloading...';
  if (state.kind === 'installing') return 'Installing...';
  if (state.kind === 'relaunching') return 'Relaunching...';
  return 'Check for updates';
}

function formatUpdateProgress(downloaded: number, total: number | null): string {
  if (!total || total <= 0) return formatBytes(downloaded);
  return `${Math.min(100, Math.round((downloaded / total) * 100))}%`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function KeybindKeys({ keybind }: { keybind: RecordingKeybind }) {
  const tokens = keybindTokens(keybind);
  return (
    <span className="shortcut-display__keys">
      {tokens.map((tok, i) => (
        <kbd key={i} className="shortcut-key">
          {tok}
        </kbd>
      ))}
    </span>
  );
}

function keybindTokens(keybind: RecordingKeybind): string[] {
  if (keybind.kind === 'fn') return ['fn'];
  if (keybind.kind === 'rightOption') return ['⌥', 'Right'];
  return keybind.label
    .split('+')
    .map((s) => s.trim())
    .filter(Boolean);
}

function keybindLabel(keybind: RecordingKeybind): string {
  if (keybind.kind === 'fn') return 'Fn';
  if (keybind.kind === 'rightOption') return 'Right Option';
  return keybind.label;
}

function keybindEqual(a: RecordingKeybind, b: RecordingKeybind): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === 'chord' && b.kind === 'chord') {
    return a.code === b.code && sameMods(a.mods, b.mods);
  }
  return true;
}

function sameMods(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const set = new Set(a);
  return b.every((m) => set.has(m));
}

const MODIFIER_CODES = new Set([
  'MetaLeft', 'MetaRight', 'OSLeft', 'OSRight',
  'ShiftLeft', 'ShiftRight',
  'AltLeft', 'AltRight',
  'ControlLeft', 'ControlRight',
  'CapsLock', 'Fn', 'FnLock',
]);

function isModifierCode(code: string): boolean {
  return MODIFIER_CODES.has(code);
}

function chordFromEvent(e: KeyboardEvent): RecordingKeybind | null {
  const mods: string[] = [];
  if (e.metaKey) mods.push('super');
  if (e.ctrlKey) mods.push('ctrl');
  if (e.altKey) mods.push('alt');
  if (e.shiftKey) mods.push('shift');

  const code = e.code;
  if (!code || isModifierCode(code)) return null;

  const label = formatChordLabel(mods, code);
  return { kind: 'chord', mods, code, label };
}

function formatChordLabel(mods: string[], code: string): string {
  const parts: string[] = [];
  if (mods.includes('ctrl')) parts.push('⌃');
  if (mods.includes('alt')) parts.push('⌥');
  if (mods.includes('shift')) parts.push('⇧');
  if (mods.includes('super')) parts.push('⌘');
  parts.push(prettyCode(code));
  return parts.join('+');
}

function prettyCode(code: string): string {
  if (code.startsWith('Key')) return code.slice(3);
  if (code.startsWith('Digit')) return code.slice(5);
  if (code.startsWith('Numpad')) return `Num${code.slice(6)}`;
  if (code.startsWith('Arrow')) {
    const dir = code.slice(5);
    return ({ Up: '↑', Down: '↓', Left: '←', Right: '→' } as Record<string, string>)[dir] ?? dir;
  }
  switch (code) {
    case 'Space': return 'Space';
    case 'Enter': return '↵';
    case 'Escape': return 'Esc';
    case 'Tab': return '⇥';
    case 'Backspace': return '⌫';
    case 'Delete': return '⌦';
    case 'Backquote': return '`';
    case 'Minus': return '-';
    case 'Equal': return '=';
    case 'BracketLeft': return '[';
    case 'BracketRight': return ']';
    case 'Backslash': return '\\';
    case 'Semicolon': return ';';
    case 'Quote': return "'";
    case 'Comma': return ',';
    case 'Period': return '.';
    case 'Slash': return '/';
    default: return code;
  }
}

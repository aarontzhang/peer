import { useEffect, useState } from 'react';
import { ipc, type AccountStatus, type AuthChangedPayload, type HotkeyStatus, type RecordingKeybind } from '@/lib/ipc';

type Props = {
  open: boolean;
  onClose: () => void;
};

const DEFAULT_KEYBIND: RecordingKeybind = { kind: 'fn' };

export function Settings({ open, onClose }: Props) {
  const [account, setAccount] = useState<AccountStatus | null>(null);
  const [pendingSignIn, setPendingSignIn] = useState(false);
  const [signInError, setSignInError] = useState<string | null>(null);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [keybind, setKeybind] = useState<RecordingKeybind>(DEFAULT_KEYBIND);
  const [initialKeybind, setInitialKeybind] = useState<RecordingKeybind>(DEFAULT_KEYBIND);
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      void ipc.getSession().then(setAccount);
      void ipc.getHotkeyStatus().then((next) => {
        setHotkey(next);
        setKeybind(next.keybind);
        setInitialKeybind(next.keybind);
      });
      setPendingSignIn(false);
      setSignInError(null);
      setCapturing(false);
      setCaptureError(null);
    }
  }, [open]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    ipc
      .onAuthChanged(async (payload: AuthChangedPayload) => {
        setAccount(await ipc.getSession());
        setPendingSignIn(false);
        if (payload.error) {
          setSignInError(payload.error);
        } else if (payload.signedIn) {
          setSignInError(null);
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
      onClose();
    } finally {
      setSaving(false);
    }
  };

  const onLogin = async () => {
    setSignInError(null);
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

  const changed = !keybindEqual(keybind, initialKeybind);
  const showHotkeyProblem =
    hotkey && keybindEqual(hotkey.keybind, keybind) && !hotkey.installed;

  return (
    <div className="settings" role="dialog" aria-modal="true" onClick={onClose}>
      <div className="settings__panel" onClick={(e) => e.stopPropagation()}>
        <h2 className="settings__title">Settings</h2>

        <section className="settings__section">
          <div className="settings__sectionHead">
            <h3 className="settings__sectionTitle">Account</h3>
            {account?.signedIn && <span className="settings__pill">Signed in</span>}
          </div>
          {account?.signedIn ? (
            <div className="settings__row">
              <span className="settings__email">{account.email ?? 'Signed in'}</span>
              <button className="btn btn--ghost" type="button" onClick={onSignOut}>
                Sign out
              </button>
            </div>
          ) : (
            <div className="settings__row">
              <button
                className="btn btn--primary"
                type="button"
                onClick={onLogin}
                disabled={pendingSignIn}
              >
                Sign in
              </button>
            </div>
          )}
          {pendingSignIn && !account?.signedIn && (
            <p className="settings__hint">Waiting for browser sign-in…</p>
          )}
          {signInError && <p className="settings__error">{signInError}</p>}
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
                className="btn btn--ghost"
                type="button"
                onClick={() => setCapturing(false)}
              >
                Cancel
              </button>
            ) : (
              <button
                className="btn btn--ghost"
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

        <div className="settings__actions">
          <button className="btn btn--ghost" onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button
            className="btn btn--primary"
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

import { useEffect, useState } from 'react';
import { ipc, type AccountStatus, type ApiKeyStatus, type HotkeyStatus, type RecordingKeybind } from '@/lib/ipc';

type Props = {
  open: boolean;
  onClose: () => void;
  onSaved: () => void;
};

export function Settings({ open, onClose, onSaved }: Props) {
  const [openai, setOpenai] = useState('');
  const [anthropic, setAnthropic] = useState('');
  const [status, setStatus] = useState<ApiKeyStatus>({ openai: false, anthropic: false });
  const [account, setAccount] = useState<AccountStatus | null>(null);
  const [deviceToken, setDeviceToken] = useState('');
  const [accountMessage, setAccountMessage] = useState<string | null>(null);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [keybind, setKeybind] = useState<RecordingKeybind>('fn');
  const [initialKeybind, setInitialKeybind] = useState<RecordingKeybind>('fn');
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      void ipc.getApiKeyStatus().then(setStatus);
      void ipc.getAccountStatus().then(setAccount);
      void ipc.getHotkeyStatus().then((next) => {
        setHotkey(next);
        setKeybind(next.keybind);
        setInitialKeybind(next.keybind);
      });
      setOpenai(''); setAnthropic(''); setDeviceToken('');
      setAccountMessage(null);
      setCapturing(false);
      setCaptureError(null);
    }
  }, [open]);

  useEffect(() => {
    if (!capturing) return;

    const onKey = (e: KeyboardEvent) => {
      const next = keybindFromEvent(e);
      e.preventDefault();
      e.stopPropagation();
      if (!next) {
        setCaptureError('Use Right Option, Fn, or Cmd+Shift+R.');
        return;
      }
      setKeybind(next);
      setCaptureError(null);
      setCapturing(false);
    };

    window.addEventListener('keydown', onKey, true);
    window.addEventListener('keyup', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('keyup', onKey, true);
    };
  }, [capturing]);

  if (!open) return null;

  const onSave = async () => {
    setSaving(true);
    try {
      if (openai.trim()) await ipc.setApiKey('openai', openai.trim());
      if (anthropic.trim()) await ipc.setApiKey('anthropic', anthropic.trim());
      if (deviceToken.trim()) {
        await ipc.setDeviceToken(deviceToken.trim());
        setAccount(await ipc.getAccountStatus());
      }
      if (keybind !== initialKeybind) {
        const next = await ipc.setRecordingKeybind(keybind);
        setHotkey(next);
        setInitialKeybind(next.keybind);
        setKeybind(next.keybind);
      }
      onSaved();
      onClose();
    } finally {
      setSaving(false);
    }
  };

  const onLogin = async () => {
    const url = await ipc.openAccountLogin();
    setAccountMessage(`Opened ${url}`);
  };

  const onSignOut = async () => {
    await ipc.signOut();
    setAccount(await ipc.getAccountStatus());
    setDeviceToken('');
    setAccountMessage(null);
  };

  const changed = !!openai.trim() || !!anthropic.trim() || !!deviceToken.trim() || keybind !== initialKeybind;
  const selectedLabel = keybindLabel(keybind);
  const showHotkeyProblem = hotkey && hotkey.keybind === keybind && !hotkey.installed;

  return (
    <div className="settings" role="dialog" aria-modal="true">
      <div className="settings__panel">
        <h2 style={{ fontSize: 17, fontWeight: 600, margin: '0 0 14px 0' }}>Settings</h2>
        <div className="field">
          <label>Peer account {account?.signedIn && <span style={{ color: 'oklch(0.74 0.16 156)' }}>· signed in</span>}</label>
          <div className="account-row">
            <button className="btn btn--primary" type="button" onClick={onLogin}>
              {account?.signedIn ? 'Open account' : 'Sign in'}
            </button>
            {account?.signedIn && (
              <button className="btn btn--ghost" type="button" onClick={onSignOut}>
                Sign out
              </button>
            )}
          </div>
          <p className="field__hint">
            Backend: {account?.backendUrl ?? 'loading'} · Device {account?.deviceId.slice(0, 8) ?? 'loading'}
          </p>
          <input
            type="password"
            placeholder="Paste device token from browser login"
            value={deviceToken}
            onChange={(e) => setDeviceToken(e.target.value)}
          />
          {accountMessage && <p className="field__hint">{accountMessage}</p>}
        </div>
        <div className="field">
          <label>OpenAI dev key {status.openai && <span style={{ color: 'oklch(0.74 0.16 156)' }}>· saved</span>}</label>
          <input
            type="password"
            placeholder={status.openai ? '••••••••' : 'sk-…'}
            value={openai}
            onChange={(e) => setOpenai(e.target.value)}
            autoFocus
          />
        </div>
        <div className="field">
          <label>Anthropic dev key {status.anthropic && <span style={{ color: 'oklch(0.74 0.16 156)' }}>· saved</span>}</label>
          <input
            type="password"
            placeholder={status.anthropic ? '••••••••' : 'sk-ant-…'}
            value={anthropic}
            onChange={(e) => setAnthropic(e.target.value)}
          />
        </div>
        <div className="field">
          <label>Recording keybind</label>
          <div className="keybind-row">
            <select
              value={keybind}
              onChange={(e) => {
                setKeybind(e.target.value as RecordingKeybind);
                setCaptureError(null);
              }}
            >
              <option value="fn">Fn</option>
              <option value="rightOption">Right Option</option>
              <option value="cmdShiftR">Cmd+Shift+R</option>
            </select>
            <button
              className="btn btn--ghost"
              type="button"
              onClick={() => {
                setCapturing(true);
                setCaptureError(null);
              }}
            >
              {capturing ? 'Press keys…' : 'Record shortcut'}
            </button>
          </div>
          <p className="field__hint">
            Tap {selectedLabel} to start or stop recording. The pill click still works.
          </p>
          {captureError && <p className="field__error">{captureError}</p>}
          {showHotkeyProblem && <p className="field__error">{hotkey.reason}</p>}
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 16 }}>
          <button className="btn btn--ghost" onClick={onClose} disabled={saving}>Cancel</button>
          <button
            className="btn btn--primary"
            onClick={onSave}
            disabled={saving || !changed}
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
        <p style={{ fontSize: 11, color: 'var(--color-fg-dim)', marginTop: 14, lineHeight: 1.5 }}>
          Account tokens are stored in macOS Keychain. Local provider keys are kept only for development fallback.
        </p>
      </div>
    </div>
  );
}

function keybindLabel(value: RecordingKeybind): string {
  switch (value) {
    case 'rightOption': return 'Right Option';
    case 'fn': return 'Fn';
    case 'cmdShiftR': return 'Cmd+Shift+R';
  }
}

function keybindFromEvent(e: KeyboardEvent): RecordingKeybind | null {
  if (e.metaKey && e.shiftKey && e.code === 'KeyR') return 'cmdShiftR';
  if (e.code === 'AltRight') return 'rightOption';
  if (e.key === 'Fn' || e.key === 'Function' || e.code === 'Fn') return 'fn';
  return null;
}

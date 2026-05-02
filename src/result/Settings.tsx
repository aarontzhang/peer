import { useEffect, useState } from 'react';
import { ipc, type ApiKeyStatus } from '@/lib/ipc';

type Props = {
  open: boolean;
  onClose: () => void;
  onSaved: () => void;
};

export function Settings({ open, onClose, onSaved }: Props) {
  const [openai, setOpenai] = useState('');
  const [anthropic, setAnthropic] = useState('');
  const [status, setStatus] = useState<ApiKeyStatus>({ openai: false, anthropic: false });
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      void ipc.getApiKeyStatus().then(setStatus);
      setOpenai(''); setAnthropic('');
    }
  }, [open]);

  if (!open) return null;

  const onSave = async () => {
    setSaving(true);
    try {
      if (openai.trim()) await ipc.setApiKey('openai', openai.trim());
      if (anthropic.trim()) await ipc.setApiKey('anthropic', anthropic.trim());
      onSaved();
      onClose();
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="settings" role="dialog" aria-modal="true">
      <div className="settings__panel">
        <h2 style={{ fontSize: 17, fontWeight: 600, margin: '0 0 14px 0' }}>API keys</h2>
        <div className="field">
          <label>OpenAI {status.openai && <span style={{ color: 'oklch(0.74 0.16 156)' }}>· saved</span>}</label>
          <input
            type="password"
            placeholder={status.openai ? '••••••••' : 'sk-…'}
            value={openai}
            onChange={(e) => setOpenai(e.target.value)}
            autoFocus
          />
        </div>
        <div className="field">
          <label>Anthropic {status.anthropic && <span style={{ color: 'oklch(0.74 0.16 156)' }}>· saved</span>}</label>
          <input
            type="password"
            placeholder={status.anthropic ? '••••••••' : 'sk-ant-…'}
            value={anthropic}
            onChange={(e) => setAnthropic(e.target.value)}
          />
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 16 }}>
          <button className="btn btn--ghost" onClick={onClose} disabled={saving}>Cancel</button>
          <button
            className="btn btn--primary"
            onClick={onSave}
            disabled={saving || (!openai.trim() && !anthropic.trim())}
          >
            {saving ? 'Saving…' : 'Save'}
          </button>
        </div>
        <p style={{ fontSize: 11, color: 'var(--color-fg-dim)', marginTop: 14, lineHeight: 1.5 }}>
          Keys are stored in macOS Keychain — they never leave your machine.
        </p>
      </div>
    </div>
  );
}

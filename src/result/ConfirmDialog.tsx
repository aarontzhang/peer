import { useEffect } from 'react';

type Props = {
  open: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  confirmDestructive?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
};

export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  confirmDestructive = false,
  busy = false,
  onConfirm,
  onCancel,
}: Props) {
  // Escape dismisses; Enter confirms. Mirrors the implicit keyboard contract
  // users expect from a confirm modal.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        if (!busy) onCancel();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (!busy) onConfirm();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, busy, onCancel, onConfirm]);

  if (!open) return null;

  const confirmClass = confirmDestructive
    ? 'btn btn--danger'
    : 'btn btn--neutral btn--neutralLight';
  const titleId = 'confirm-dialog-title';

  return (
    <div
      className="settings"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      onClick={() => !busy && onCancel()}
    >
      <div className="settings__panel confirm__panel" onClick={(e) => e.stopPropagation()}>
        <h2 className="settings__title" id={titleId}>{title}</h2>
        <p className="confirm__message">{message}</p>
        <div className="settings__actions">
          <button
            className="btn btn--neutral btn--neutralDark"
            onClick={onCancel}
            disabled={busy}
            type="button"
          >
            {cancelLabel}
          </button>
          <button
            className={confirmClass}
            onClick={onConfirm}
            disabled={busy}
            type="button"
            autoFocus
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

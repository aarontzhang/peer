import { useCallback, useEffect, useMemo, useState } from 'react';
import { ipc, type RecordingVersion, type VersionSource, formatRelative } from '@/lib/ipc';

type Props = {
  recordingId: string;
  open: boolean;
  /** Bumped externally when a new version is appended (chat, retry, revert). */
  refreshKey: number;
  onClose: () => void;
  /** Called when the user reverts; parent triggers list refresh. */
  onReverted: () => void;
};

const SOURCE_LABEL: Record<VersionSource, string> = {
  initial: 'Initial',
  chat: 'Chat',
  retry: 'Retry',
  revert: 'Reverted',
};

export function VersionHistoryPanel({
  recordingId,
  open,
  refreshKey,
  onClose,
  onReverted,
}: Props) {
  const [versions, setVersions] = useState<RecordingVersion[]>([]);
  const [loading, setLoading] = useState(false);
  const [reverting, setReverting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await ipc.listVersions(recordingId);
      setVersions(list);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [recordingId]);

  useEffect(() => {
    if (!open) return;
    void load();
  }, [open, load, refreshKey]);

  const currentVersionId = useMemo(() => versions[0]?.id ?? null, [versions]);

  const onRevert = useCallback(
    async (version: RecordingVersion) => {
      if (reverting) return;
      setReverting(version.id);
      setError(null);
      try {
        await ipc.revertToVersion(version.id);
        await load();
        onReverted();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setReverting(null);
      }
    },
    [reverting, load, onReverted],
  );

  if (!open) return null;

  return (
    <aside
      className="version-panel"
      role="complementary"
      aria-label="Prompt version history"
    >
      <header className="version-panel__head">
        <span className="version-panel__title">History</span>
        <button
          type="button"
          className="version-panel__close"
          onClick={onClose}
          aria-label="Close history"
          title="Close"
        >
          <CloseIcon />
        </button>
      </header>
      {error && <p className="version-panel__error">{error}</p>}
      <div className="version-panel__list">
        {loading && versions.length === 0 ? (
          <p className="version-panel__muted">Loading…</p>
        ) : versions.length === 0 ? (
          <p className="version-panel__muted">No versions yet.</p>
        ) : (
          versions.map((v) => {
            const isCurrent = v.id === currentVersionId;
            const messagePreview = v.source === 'chat' && v.sourceMessageContent
              ? truncate(v.sourceMessageContent, 80)
              : null;
            return (
              <article
                key={v.id}
                className={`version-row${isCurrent ? ' version-row--current' : ''}`}
              >
                <header className="version-row__head">
                  <span className={`version-pill version-pill--${v.source}`}>
                    {SOURCE_LABEL[v.source]}
                  </span>
                  <span className="version-row__meta">
                    v{v.versionNo} · {formatRelative(v.createdAt)}
                  </span>
                </header>
                {messagePreview && (
                  <p className="version-row__msg">“{messagePreview}”</p>
                )}
                <p className="version-row__body">{truncate(v.body, 220)}</p>
                {!isCurrent && (
                  <button
                    type="button"
                    className="version-row__action"
                    onClick={() => void onRevert(v)}
                    disabled={reverting !== null}
                  >
                    {reverting === v.id ? 'Reverting…' : 'Revert to this'}
                  </button>
                )}
                {isCurrent && (
                  <span className="version-row__current">Current</span>
                )}
              </article>
            );
          })
        )}
      </div>
    </aside>
  );
}

function truncate(s: string, max: number): string {
  const clean = s.replace(/\s+/g, ' ').trim();
  if (clean.length <= max) return clean;
  return clean.slice(0, max - 1).trimEnd() + '…';
}

function CloseIcon() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path
        d="M3.5 3.5l9 9M12.5 3.5l-9 9"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}

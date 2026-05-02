type Props = {
  needsKeys: boolean;
  onOpenSettings: () => void;
};

export function EmptyState({ needsKeys, onOpenSettings }: Props) {
  return (
    <div className="main">
      <div className="main__bar" data-tauri-drag-region>
        {needsKeys && (
          <div className="main__actions">
            <button className="btn btn--primary" onClick={onOpenSettings}>
              Add API keys
            </button>
          </div>
        )}
      </div>
      <div className="empty">
        <div>
          <div className="empty__title">Show, don't tell.</div>
          <div className="empty__sub">
            Click the orb on the floating pill to start recording. Click again
            to stop. Peer turns your screen + narration into a
            paste-ready instruction set for Claude Code.
          </div>
        </div>
      </div>
    </div>
  );
}

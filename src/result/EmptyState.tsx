export function EmptyState() {
  return (
    <div className="main">
      <div className="main__bar" data-tauri-drag-region />
      <div className="empty">
        <div>
          <div className="empty__title">Show, don't tell.</div>
          <div className="empty__sub">
            Click the orb on the floating pill to start recording. Click again
            to stop. Peer turns your screen + narration into a paste-ready
            instruction set for Claude Code.
          </div>
        </div>
      </div>
    </div>
  );
}

type Props = {
  onOpenSettings: () => void;
};

export function SidebarAccount({ onOpenSettings }: Props) {
  return (
    <div className="sidebar__footer" data-no-drag>
      <button
        type="button"
        className="account-trigger account-trigger--gear"
        onClick={onOpenSettings}
        aria-label="Open settings"
      >
        <GearIcon />
        <span className="account-trigger__name">Settings</span>
      </button>
    </div>
  );
}

function GearIcon() {
  // 8-tooth gear, drawn so the body and the teeth are clearly distinct
  // and the silhouette doesn't read as a flower at small sizes.
  return (
    <svg
      className="account-trigger__gear-icon"
      width="15"
      height="15"
      viewBox="0 0 24 24"
      aria-hidden
    >
      <path
        d="M19.4 13a7.5 7.5 0 0 0 0-2l2-1.5-2-3.5-2.4.8a7.6 7.6 0 0 0-1.7-1l-.4-2.5h-4l-.4 2.5a7.6 7.6 0 0 0-1.7 1l-2.4-.8-2 3.5 2 1.5a7.5 7.5 0 0 0 0 2l-2 1.5 2 3.5 2.4-.8a7.6 7.6 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7.6 7.6 0 0 0 1.7-1l2.4.8 2-3.5z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

import { useApp } from "../../app/view";
import { ProtectionBadge } from "../ProtectionBadge";
import { StatusDot } from "../common/StatusDot";

export function Header() {
  const { view, actions } = useApp();
  const chip = [view.providerName, view.activeProfile?.name].filter(Boolean).join(" · ");
  return (
    <header className="app-header">
      <div className="app-header__row">
        <StatusDot view={view} />
        <h1 className="app-header__title">AI Call Assistant</h1>
        {chip && (
          <span className="app-header__chip" title={chip} data-testid="provider-chip">
            {chip}
          </span>
        )}
        <div className="app-header__actions">
          <button
            type="button"
            className="icon-btn"
            aria-label="Enter prompter mode"
            title="Enter prompter mode"
            onClick={() => actions.setLayout("prompter")}
          >
            ⤒
          </button>
          <button type="button" className="icon-btn" aria-label="Dock under camera" title="Dock under camera" onClick={() => actions.dock()}>
            ⊤
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="Settings"
            title="Settings"
            disabled={!view.settings}
            onClick={() => actions.openSettings()}
          >
            ⚙
          </button>
        </div>
      </div>
      <ProtectionBadge />
    </header>
  );
}

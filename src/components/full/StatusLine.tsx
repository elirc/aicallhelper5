import { useApp } from "../../app/view";
import { RecordingTimer } from "../common/RecordingInfo";

/** One-line status (aria-live polite) + timer + first-run Settings prompt. */
export function StatusLine({ compact = false }: { compact?: boolean }) {
  const { view, actions } = useApp();
  return (
    <div className={`status-line${compact ? " status-line--compact" : ""}`}>
      <p className="status-line__text" aria-live="polite" data-testid="status-text">
        {view.statusText}
      </p>
      <RecordingTimer />
      {view.needsSetup && (
        <button type="button" className="btn btn--small btn--primary" onClick={() => actions.openSettings()}>
          Open Settings
        </button>
      )}
    </div>
  );
}

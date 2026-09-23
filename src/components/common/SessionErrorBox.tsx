import { useApp } from "../../app/view";
import { SETTINGS_ERROR_CODES } from "./labels";

export function SessionErrorBox() {
  const { view, actions } = useApp();
  const err = view.sessionError;
  if (!err) return null;
  return (
    <div className="alert alert--error" role="alert" data-testid="session-error">
      <p className="alert__text">{err.message}</p>
      {SETTINGS_ERROR_CODES.has(err.code) && (
        <button type="button" className="btn btn--small" onClick={() => actions.openSettings()}>
          Open Settings
        </button>
      )}
    </div>
  );
}

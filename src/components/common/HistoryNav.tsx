import { useApp } from "../../app/view";
import { callTypeLabel } from "./labels";

/** ← n/m → history arrows (+ call-type tag and Clear in the full layout). */
export function HistoryNav({ compact = false }: { compact?: boolean }) {
  const { view, actions } = useApp();
  const { history } = view;
  if (history.count < 1) return null;
  return (
    <nav className={`history${compact ? " history--compact" : ""}`} aria-label="Answer history">
      <button
        type="button"
        className="icon-btn"
        aria-label="Previous answer"
        title="Previous answer"
        disabled={!history.canPrev}
        onClick={() => actions.historyPrev()}
      >
        ←
      </button>
      <span className="history__pos" data-testid="history-pos">
        {history.index}/{history.count}
      </span>
      <button
        type="button"
        className="icon-btn"
        aria-label="Next answer"
        title="Next answer"
        disabled={!history.canNext}
        onClick={() => actions.historyNext()}
      >
        →
      </button>
      {!compact && view.entry && (
        <span className="tag" data-testid="history-tag">
          {callTypeLabel(view.entry.callType)}
        </span>
      )}
      {!compact && (
        <button type="button" className="btn btn--ghost btn--small history__clear" onClick={() => actions.clearHistory()}>
          Clear
        </button>
      )}
    </nav>
  );
}

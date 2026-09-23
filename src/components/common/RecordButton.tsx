import { useApp } from "../../app/view";

/**
 * The one record control. The SAME element stays mounted across phases so
 * focus is kept when it turns into "Finalizing…" (aria-disabled, never the
 * `disabled` attribute, which would drop focus).
 */
export function RecordButton({ compact = false }: { compact?: boolean }) {
  const { view, actions } = useApp();
  const coreReady = view.core === "ready";
  const { phase } = view;

  let label: string;
  let disabled = false;
  let busy = false;
  let run: () => void;
  let mod: string;

  if (phase === "starting" || phase === "recording") {
    label = "Stop & Answer";
    disabled = !view.canStop;
    run = () => actions.stop();
    mod = "stop";
  } else if (phase === "finalizing") {
    label = "Finalizing…";
    busy = true;
    run = () => {};
    mod = "busy";
  } else {
    // idle or answering: a new Record supersedes a streaming answer.
    label = "Record";
    disabled = !view.canRecord || !coreReady;
    run = () => actions.record();
    mod = "record";
  }

  return (
    <button
      type="button"
      className={`record-btn record-btn--${mod}${compact ? " record-btn--compact" : ""}`}
      disabled={disabled}
      aria-disabled={busy ? true : undefined}
      data-testid="record-button"
      onClick={() => {
        if (busy) return;
        run();
      }}
    >
      <span className="record-btn__dot" aria-hidden="true" />
      {label}
    </button>
  );
}

import { useApp } from "../../app/view";
import { countdownSeconds, formatClock } from "./format";
import { SILENT_NOTE } from "./labels";

/** Elapsed recording timer (re-rendered by the provider's ticking clock). */
export function RecordingTimer() {
  const { view } = useApp();
  if (!view.recording) return null;
  const elapsed = formatClock(Date.now() - view.recording.startedAtMs);
  return (
    <span className="rec-timer" data-testid="rec-timer">
      <span className="rec-timer__dot" aria-hidden="true" />
      <span className="visually-hidden">Recording </span>
      {elapsed}
    </span>
  );
}

/** Countdown shown only in the last 30 s before the core's auto-stop deadline. */
export function Countdown() {
  const { view } = useApp();
  if (!view.recording) return null;
  const secs = countdownSeconds(view.recording.deadlineMs, Date.now());
  if (secs === null) return null;
  return (
    <span className="countdown" data-testid="countdown">
      Auto-stop in {secs} s
    </span>
  );
}

export function LevelMeter() {
  const { view } = useApp();
  if (!view.recording) return null;
  const pct = Math.round(Math.min(1, Math.max(0, view.recording.level)) * 100);
  return (
    <div className="meter-wrap">
      <div className="meter" role="meter" aria-label="Call audio level" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
        <div className="meter__fill" style={{ width: `${pct}%` }} />
      </div>
      <Countdown />
    </div>
  );
}

export function SilenceNote() {
  const { view } = useApp();
  if (!view.recording?.silent) return null;
  return (
    <p className="silence-note" data-testid="silence-note">
      {SILENT_NOTE}
    </p>
  );
}

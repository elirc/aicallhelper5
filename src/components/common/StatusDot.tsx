import type { AppView } from "../../app/view";

export type DotTone = "ok" | "warn" | "bad" | "rec" | "busy";

export function dotTone(view: Pick<AppView, "core" | "phase">): DotTone {
  if (view.core === "failed") return "bad";
  if (view.core === "starting") return "warn";
  switch (view.phase) {
    case "recording":
      return "rec";
    case "starting":
    case "finalizing":
    case "answering":
      return "busy";
    default:
      return "ok";
  }
}

const TONE_LABEL: Record<DotTone, string> = {
  ok: "Ready",
  warn: "Starting",
  bad: "Core failed",
  rec: "Recording",
  busy: "Working",
};

export function StatusDot({ view }: { view: Pick<AppView, "core" | "phase"> }) {
  const tone = dotTone(view);
  return <span className={`status-dot status-dot--${tone}`} role="img" aria-label={TONE_LABEL[tone]} data-tone={tone} />;
}

import { useLayoutEffect, useRef } from "react";
import type { AnswerEntryView } from "../../app/view";

/**
 * Calls `report()` once per streaming answer, in a layout effect, the first
 * time the streaming entry has non-empty answer text — i.e. right as the
 * first word is committed to the DOM (spec §15 "visible first word").
 * Re-arms when the entry stops streaming or its answer is empty (new session).
 */
export function useFirstPaint(entry: AnswerEntryView | null, report: () => void): void {
  const reported = useRef(false);
  const streaming = entry?.streaming ?? false;
  const hasText = (entry?.answer.length ?? 0) > 0;
  useLayoutEffect(() => {
    if (!streaming || !hasText) {
      reported.current = false;
      return;
    }
    if (!reported.current) {
      reported.current = true;
      report();
    }
  }, [streaming, hasText, report]);
}

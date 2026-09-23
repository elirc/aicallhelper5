import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useApp } from "../../app/view";
import { Markdown } from "../../markdown";
import { ProtectionBadge } from "../ProtectionBadge";
import { HistoryNav } from "../common/HistoryNav";
import { RecordButton } from "../common/RecordButton";
import { Countdown, RecordingTimer } from "../common/RecordingInfo";
import { SessionErrorBox } from "../common/SessionErrorBox";
import { StyleChips } from "../common/StyleChips";
import { useFirstPaint } from "../common/useFirstPaint";
import { FinishNote } from "../full/AnswerPanel";

/**
 * Prompter strip: only the answer, large, anchored at the TOP. It never
 * auto-scrolls while streaming; a "▼ more" hint appears when text overflows.
 */
export function PrompterLayout() {
  const { view, actions } = useApp();
  const entry = view.entry;
  const scrollRef = useRef<HTMLDivElement>(null);
  const [more, setMore] = useState(false);

  const reportFirstPaint = useCallback(() => actions.reportFirstPaint(), [actions]);
  useFirstPaint(entry, reportFirstPaint);

  const measure = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setMore(el.scrollHeight - el.scrollTop - el.clientHeight > 4);
  }, []);

  useLayoutEffect(() => {
    measure();
  }, [entry?.answer, view.prompterFontPx, measure]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && !e.defaultPrevented) {
        e.preventDefault();
        actions.setLayout("full");
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [actions]);

  function scrollMore() {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollTop + Math.max(40, Math.floor(el.clientHeight * 0.8));
    measure();
  }

  const question = view.phase !== "idle" ? view.liveTranscript : (entry?.question ?? "");
  const streaming = entry?.streaming ?? false;

  return (
    <div className="layout layout--prompter" data-testid="prompter-layout">
      <div className="prompter-top">
        <p className="prompter-status" aria-live="polite" data-testid="status-text">
          {view.statusText}
        </p>
        <RecordingTimer />
        <Countdown />
        <ProtectionBadge compact />
      </div>
      {view.core === "failed" && (
        <div className="alert alert--error" role="alert">
          <p className="alert__text">{view.coreError?.message ?? "The app core failed to start."}</p>
        </div>
      )}
      <SessionErrorBox />
      <div className="prompter-body">
        <div
          ref={scrollRef}
          className="prompter-scroll"
          onScroll={measure}
          aria-live="polite"
          aria-busy={streaming}
          aria-label="Suggested answer"
          data-testid="prompter-scroll"
          style={{ fontSize: `${view.prompterFontPx}px` }}
        >
          <div className="prompter-column">
            {entry && entry.answer !== "" ? (
              <Markdown source={entry.answer} className="answer-md" />
            ) : (
              <p className="empty-hint">The answer appears here in large text.</p>
            )}
            {entry && <FinishNote entry={entry} />}
          </div>
        </div>
        {more && (
          <button type="button" className="more-hint" data-testid="more-hint" onClick={scrollMore} aria-label="Scroll down for more of the answer">
            ▼ more
          </button>
        )}
      </div>
      <div className="prompter-controls">
        <RecordButton compact />
        <p className="prompter-question" title={question} data-testid="prompter-question">
          {question || " "}
        </p>
        <StyleChips compact />
        <HistoryNav compact />
        <button type="button" className="icon-btn icon-btn--text" aria-label="Smaller prompter text" title="Smaller prompter text" onClick={() => actions.bumpPrompterFont(-1)}>
          A−
        </button>
        <button type="button" className="icon-btn icon-btn--text" aria-label="Larger prompter text" title="Larger prompter text" onClick={() => actions.bumpPrompterFont(1)}>
          A+
        </button>
        <button type="button" className="icon-btn" aria-label="Re-dock under camera" title="Re-dock under camera" onClick={() => actions.dock()}>
          ⤒
        </button>
        <button type="button" className="icon-btn" aria-label="Exit prompter mode (Esc)" title="Exit prompter mode (Esc)" onClick={() => actions.setLayout("full")}>
          ⤢
        </button>
      </div>
    </div>
  );
}

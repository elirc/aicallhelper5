import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useApp, type AnswerEntryView } from "../../app/view";
import { Markdown } from "../../markdown";
import { copyText } from "../common/clipboard";
import { REFUSED_NOTE, TRUNCATED_NOTE } from "../common/labels";
import { SessionErrorBox } from "../common/SessionErrorBox";
import { useFirstPaint } from "../common/useFirstPaint";

/** Pixels from the bottom that still count as "at the bottom". */
const STICK_SLACK_PX = 24;

export function latencyTooltip(entry: AnswerEntryView): string {
  const m = entry.metrics;
  if (!m) return "";
  const visible = entry.visibleFirstWordMs === null ? "—" : `${entry.visibleFirstWordMs} ms`;
  return [
    `First word (core): ${m.firstTokenMs} ms`,
    `Audio drain: ${m.audioDrainMs} ms`,
    `Transcript finalize: ${m.sttFinalizeMs} ms`,
    `Total answer: ${m.totalMs} ms`,
    `Visible first word: ${visible}`,
  ].join("\n");
}

export function FinishNote({ entry }: { entry: AnswerEntryView }) {
  if (entry.finish === "truncated") {
    return (
      <p className="finish-note" data-testid="finish-note">
        {TRUNCATED_NOTE}
      </p>
    );
  }
  if (entry.finish === "refused") {
    return (
      <p className="finish-note" data-testid="finish-note">
        {REFUSED_NOTE}
      </p>
    );
  }
  return null;
}

type CopyState = "idle" | "copied" | "failed";

export function AnswerPanel() {
  const { view, actions } = useApp();
  const entry = view.entry;
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const reportFirstPaint = useCallback(() => actions.reportFirstPaint(), [actions]);
  useFirstPaint(entry, reportFirstPaint);

  const streaming = entry?.streaming ?? false;
  const answer = entry?.answer ?? "";

  // A fresh stream starts pinned to the bottom again.
  useLayoutEffect(() => {
    if (streaming && answer === "") stick.current = true;
  }, [streaming, answer]);

  // Sticky auto-scroll while streaming, unless the user scrolled up.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || !streaming || !stick.current) return;
    el.scrollTop = el.scrollHeight;
  }, [answer, streaming]);

  useEffect(
    () => () => {
      if (copyTimer.current) clearTimeout(copyTimer.current);
    },
    [],
  );

  function onScroll() {
    const el = scrollRef.current;
    if (!el) return;
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight <= STICK_SLACK_PX;
  }

  async function onCopy() {
    if (!entry) return;
    const ok = await copyText(entry.answer);
    setCopyState(ok ? "copied" : "failed");
    if (copyTimer.current) clearTimeout(copyTimer.current);
    copyTimer.current = setTimeout(() => setCopyState("idle"), 1500);
  }

  return (
    <section className="panel panel--answer" aria-labelledby="answer-heading">
      <div className="panel__head">
        <h2 id="answer-heading" className="panel__title">
          Suggested answer
        </h2>
        <div className="panel__tools">
          {entry?.metrics && (
            <span className="latency-chip" title={latencyTooltip(entry)} data-testid="latency-chip" tabIndex={0}>
              first word in {entry.metrics.firstTokenMs} ms
            </span>
          )}
          <button type="button" className="icon-btn icon-btn--text" aria-label="Smaller answer text" title="Smaller answer text" onClick={() => actions.bumpAnswerFont(-1)}>
            A−
          </button>
          <button type="button" className="icon-btn icon-btn--text" aria-label="Larger answer text" title="Larger answer text" onClick={() => actions.bumpAnswerFont(1)}>
            A+
          </button>
          <button type="button" className="btn btn--small" disabled={!view.canRegenerate} onClick={() => actions.regenerate()}>
            Regenerate
          </button>
          <button type="button" className="btn btn--small" disabled={!entry || entry.answer === ""} onClick={() => void onCopy()}>
            {copyState === "copied" ? "Copied" : copyState === "failed" ? "Copy failed" : "Copy"}
          </button>
        </div>
      </div>
      <SessionErrorBox />
      <div
        ref={scrollRef}
        className="answer-scroll"
        onScroll={onScroll}
        aria-live="polite"
        aria-busy={streaming}
        data-testid="answer-scroll"
        style={{ fontSize: `${view.answerFontPx}px` }}
      >
        {entry && entry.answer !== "" ? (
          <Markdown source={entry.answer} className="answer-md" />
        ) : (
          <p className="empty-hint">
            {view.phase === "answering" || view.phase === "finalizing"
              ? "The answer will appear here…"
              : "Press Record while the other person asks a question, then Stop & Answer. Or type a question below."}
          </p>
        )}
        {entry && <FinishNote entry={entry} />}
      </div>
      <span className="visually-hidden" aria-live="polite">
        {copyState === "copied" ? "Answer copied" : ""}
      </span>
    </section>
  );
}

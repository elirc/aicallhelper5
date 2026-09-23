// Test-only helpers for component tests (not imported by production code).
import { render, type RenderResult } from "@testing-library/react";
import type { ReactElement } from "react";
import { vi } from "vitest";
import { makeActions, makeView } from "../app/testing";
import { AppContext, type AnswerEntryView, type AppActions, type AppView, type RecordingView } from "../app/view";

export function makeEntry(over: Partial<AnswerEntryView> = {}): AnswerEntryView {
  return {
    question: "Tell me about yourself",
    answer: "I am a backend engineer.",
    streaming: false,
    finish: "complete",
    callType: "behavioral",
    metrics: { audioDrainMs: 40, sttFinalizeMs: 120, firstTokenMs: 850, totalMs: 2400 },
    visibleFirstWordMs: 870,
    error: null,
    ...over,
  };
}

export function makeRecording(over: Partial<RecordingView> = {}): RecordingView {
  const now = Date.now();
  return { deadlineMs: now + 110_000, capMs: 120_000, startedAtMs: now - 10_000, level: 0.4, silent: false, ...over };
}

export interface Harness extends RenderResult {
  actions: AppActions;
  /** Re-render with a new view, keeping the same actions object. */
  update(over: Partial<AppView>): void;
}

export function mocked(fn: unknown) {
  return fn as ReturnType<typeof vi.fn>;
}

export function renderWithView(ui: ReactElement, over: Partial<AppView> = {}, actions?: AppActions): Harness {
  const acts = actions ?? makeActions(() => vi.fn());
  const wrap = (v: AppView) => <AppContext.Provider value={{ view: v, actions: acts }}>{ui}</AppContext.Provider>;
  const result = render(wrap(makeView(over)));
  return {
    ...result,
    actions: acts,
    update(next: Partial<AppView>) {
      result.rerender(wrap(makeView(next)));
    },
  };
}

/** Pretend an element has a given scroll geometry (jsdom has no layout). */
export function setScrollGeometry(el: HTMLElement, g: { scrollHeight: number; clientHeight: number }) {
  Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => g.scrollHeight });
  Object.defineProperty(el, "clientHeight", { configurable: true, get: () => g.clientHeight });
}

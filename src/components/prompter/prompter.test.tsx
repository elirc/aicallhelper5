import { fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { makeEntry, makeRecording, renderWithView, setScrollGeometry } from "../testUtils";
import { PrompterLayout } from "./PrompterLayout";

vi.mock("../../markdown", () => ({
  Markdown: ({ source }: { source: string }) => <div data-testid="md">{source}</div>,
}));

afterEach(() => vi.restoreAllMocks());

const prompter = { layout: "prompter" as const };

describe("PrompterLayout", () => {
  it("prompter_shows_answer_at_prompter_font", () => {
    renderWithView(<PrompterLayout />, { ...prompter, entry: makeEntry({ answer: "Big words" }), prompterFontPx: 22 });
    expect(screen.getByTestId("md")).toHaveTextContent("Big words");
    expect(screen.getByTestId("prompter-scroll").style.fontSize).toBe("22px");
  });

  it("prompter_escape_exits_to_full", async () => {
    const h = renderWithView(<PrompterLayout />, prompter);
    await userEvent.keyboard("{Escape}");
    expect(h.actions.setLayout).toHaveBeenCalledWith("full");
  });

  it("prompter_exit_and_redock_buttons", async () => {
    const h = renderWithView(<PrompterLayout />, prompter);
    await userEvent.click(screen.getByRole("button", { name: "Re-dock under camera" }));
    await userEvent.click(screen.getByRole("button", { name: "Exit prompter mode (Esc)" }));
    expect(h.actions.dock).toHaveBeenCalledTimes(1);
    expect(h.actions.setLayout).toHaveBeenCalledWith("full");
  });

  it("prompter_font_buttons", async () => {
    const h = renderWithView(<PrompterLayout />, prompter);
    await userEvent.click(screen.getByRole("button", { name: "Smaller prompter text" }));
    await userEvent.click(screen.getByRole("button", { name: "Larger prompter text" }));
    expect(h.actions.bumpPrompterFont).toHaveBeenNthCalledWith(1, -1);
    expect(h.actions.bumpPrompterFont).toHaveBeenNthCalledWith(2, 1);
  });

  it("prompter_does_not_auto_scroll_while_streaming", () => {
    const v = (answer: string) => ({ ...prompter, phase: "answering" as const, entry: makeEntry({ streaming: true, finish: null, answer }) });
    const h = renderWithView(<PrompterLayout />, v("First"));
    const el = screen.getByTestId("prompter-scroll");
    setScrollGeometry(el, { scrollHeight: 900, clientHeight: 100 });
    h.update(v("First second"));
    h.update(v("First second third"));
    expect(el.scrollTop).toBe(0);
  });

  it("prompter_more_hint_when_overflowing", async () => {
    const v = (answer: string) => ({ ...prompter, entry: makeEntry({ answer }) });
    const h = renderWithView(<PrompterLayout />, v("short"));
    expect(screen.queryByTestId("more-hint")).toBeNull();
    const el = screen.getByTestId("prompter-scroll");
    setScrollGeometry(el, { scrollHeight: 500, clientHeight: 100 });
    h.update(v("a much longer answer"));
    const hint = screen.getByTestId("more-hint");
    expect(hint).toHaveTextContent("▼ more");
    await userEvent.click(hint);
    expect(el.scrollTop).toBeGreaterThan(0);
    // scrolled to the bottom → hint goes away
    el.scrollTop = 400;
    fireEvent.scroll(el);
    expect(screen.queryByTestId("more-hint")).toBeNull();
  });

  it("prompter_status_line_includes_finalizing", () => {
    renderWithView(<PrompterLayout />, { ...prompter, phase: "finalizing", statusText: "Finalizing transcript…" });
    const s = screen.getByTestId("status-text");
    expect(s).toHaveTextContent("Finalizing transcript…");
    expect(s).toHaveAttribute("aria-live", "polite");
    expect(screen.getByRole("button", { name: "Finalizing…" })).toHaveAttribute("aria-disabled", "true");
  });

  it("prompter_record_button_and_timer", async () => {
    const h = renderWithView(<PrompterLayout />, { ...prompter, phase: "recording", canStop: true, recording: makeRecording() });
    expect(screen.getByTestId("rec-timer")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Stop & Answer" }));
    expect(h.actions.stop).toHaveBeenCalledTimes(1);
  });

  it("prompter_one_line_question_live_or_entry", () => {
    const h = renderWithView(<PrompterLayout />, { ...prompter, phase: "recording", liveTranscript: "How do you scale", recording: makeRecording() });
    expect(screen.getByTestId("prompter-question")).toHaveTextContent("How do you scale");
    h.update({ ...prompter, phase: "idle", entry: makeEntry({ question: "Old question" }) });
    expect(screen.getByTestId("prompter-question")).toHaveTextContent("Old question");
    expect(screen.getByTestId("prompter-question")).toHaveAttribute("title", "Old question");
  });

  it("prompter_has_style_chips_and_history", async () => {
    const h = renderWithView(<PrompterLayout />, {
      ...prompter,
      entry: makeEntry(),
      history: { index: 1, count: 2, canPrev: false, canNext: true },
    });
    expect(screen.getByRole("radiogroup", { name: "Answer style" })).toBeInTheDocument();
    expect(screen.getByTestId("history-pos")).toHaveTextContent("1/2");
    await userEvent.click(screen.getByRole("button", { name: "Next answer" }));
    expect(h.actions.historyNext).toHaveBeenCalledTimes(1);
  });

  it("prompter_reports_first_paint", () => {
    const h = renderWithView(<PrompterLayout />, { ...prompter, phase: "answering", entry: makeEntry({ streaming: true, answer: "", finish: null }) });
    h.update({ ...prompter, phase: "answering", entry: makeEntry({ streaming: true, answer: "Yes", finish: null }) });
    expect(h.actions.reportFirstPaint).toHaveBeenCalledTimes(1);
  });

  it("prompter_truncated_note", () => {
    renderWithView(<PrompterLayout />, { ...prompter, entry: makeEntry({ finish: "truncated" }) });
    expect(screen.getByTestId("finish-note")).toHaveTextContent("cut off");
  });

  it("prompter_session_error_alert", () => {
    renderWithView(<PrompterLayout />, { ...prompter, sessionError: { code: "stt_connect", message: "Couldn't reach Deepgram." } });
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't reach Deepgram.");
  });
});

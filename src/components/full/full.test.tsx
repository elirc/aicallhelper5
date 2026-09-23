import { fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { makeSettings } from "../../app/testing";
import { makeEntry, makeRecording, renderWithView, setScrollGeometry } from "../testUtils";
import { AnswerPanel, latencyTooltip } from "./AnswerPanel";
import { AskBox, NoticeBar, ProfileControls, QuestionPanel } from "./Controls";
import { FullLayout } from "./FullLayout";
import { Header } from "./Header";
import { StatusLine } from "./StatusLine";

vi.mock("../../markdown", () => ({
  Markdown: ({ source, className }: { source: string; className?: string }) => (
    <div data-testid="md" className={className}>
      {source}
    </div>
  ),
}));

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Header", () => {
  it("header_chip_and_buttons", async () => {
    const h = renderWithView(<Header />);
    expect(screen.getByTestId("provider-chip")).toHaveTextContent("Claude Haiku 4.5 (recommended) · Default");
    await userEvent.click(screen.getByRole("button", { name: "Enter prompter mode" }));
    await userEvent.click(screen.getByRole("button", { name: "Dock under camera" }));
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(h.actions.setLayout).toHaveBeenCalledWith("prompter");
    expect(h.actions.dock).toHaveBeenCalledTimes(1);
    expect(h.actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it("icon_buttons_have_title_and_label", () => {
    renderWithView(<Header />);
    for (const name of ["Enter prompter mode", "Dock under camera", "Settings"]) {
      expect(screen.getByRole("button", { name })).toHaveAttribute("title", name);
    }
  });
});

describe("StatusLine", () => {
  it("status_line_is_polite_live_region", () => {
    renderWithView(<StatusLine />, { statusText: "Finalizing transcript…" });
    const s = screen.getByTestId("status-text");
    expect(s).toHaveTextContent("Finalizing transcript…");
    expect(s).toHaveAttribute("aria-live", "polite");
  });

  it("status_line_needs_setup_opens_settings", async () => {
    const h = renderWithView(<StatusLine />, { needsSetup: true, statusText: "Add your API keys — open Settings…" });
    await userEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(h.actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it("status_line_shows_timer_while_recording", () => {
    renderWithView(<StatusLine />, { phase: "recording", recording: makeRecording() });
    expect(screen.getByTestId("rec-timer")).toBeInTheDocument();
  });
});

describe("AnswerPanel", () => {
  it("answer_renders_markdown_at_font_size", () => {
    renderWithView(<AnswerPanel />, { entry: makeEntry({ answer: "**Yes.**" }), answerFontPx: 16 });
    expect(screen.getByTestId("md")).toHaveTextContent("**Yes.**");
    expect(screen.getByTestId("answer-scroll").style.fontSize).toBe("16px");
  });

  it("answer_empty_state_hint", () => {
    renderWithView(<AnswerPanel />, { entry: null });
    expect(screen.getByText(/Press Record while the other person asks a question/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
  });

  it("answer_font_buttons_bump", async () => {
    const h = renderWithView(<AnswerPanel />, { entry: makeEntry() });
    await userEvent.click(screen.getByRole("button", { name: "Smaller answer text" }));
    await userEvent.click(screen.getByRole("button", { name: "Larger answer text" }));
    expect(h.actions.bumpAnswerFont).toHaveBeenNthCalledWith(1, -1);
    expect(h.actions.bumpAnswerFont).toHaveBeenNthCalledWith(2, 1);
  });

  it("regenerate_follows_canRegenerate", async () => {
    const h = renderWithView(<AnswerPanel />, { entry: makeEntry(), canRegenerate: false });
    expect(screen.getByRole("button", { name: "Regenerate" })).toBeDisabled();
    h.update({ entry: makeEntry(), canRegenerate: true });
    await userEvent.click(screen.getByRole("button", { name: "Regenerate" }));
    expect(h.actions.regenerate).toHaveBeenCalledTimes(1);
  });

  it("latency_chip_and_tooltip", () => {
    const entry = makeEntry({
      metrics: { audioDrainMs: 41, sttFinalizeMs: 130, firstTokenMs: 912, totalMs: 2500 },
      visibleFirstWordMs: 940,
    });
    renderWithView(<AnswerPanel />, { entry });
    const chip = screen.getByTestId("latency-chip");
    expect(chip).toHaveTextContent("first word in 912 ms");
    const title = chip.getAttribute("title") ?? "";
    expect(title).toContain("912 ms");
    expect(title).toContain("Audio drain: 41 ms");
    expect(title).toContain("Transcript finalize: 130 ms");
    expect(title).toContain("Total answer: 2500 ms");
    expect(title).toContain("Visible first word: 940 ms");
  });

  it("latency_tooltip_without_visible_measure", () => {
    expect(latencyTooltip(makeEntry({ visibleFirstWordMs: null }))).toContain("Visible first word: —");
  });

  it("latency_chip_absent_without_metrics", () => {
    renderWithView(<AnswerPanel />, { entry: makeEntry({ metrics: null, streaming: true, finish: null }) });
    expect(screen.queryByTestId("latency-chip")).toBeNull();
  });

  it("truncated_note", () => {
    renderWithView(<AnswerPanel />, { entry: makeEntry({ finish: "truncated" }) });
    expect(screen.getByTestId("finish-note")).toHaveTextContent("The answer was cut off at the length limit.");
  });

  it("refused_note", () => {
    renderWithView(<AnswerPanel />, { entry: makeEntry({ finish: "refused" }) });
    expect(screen.getByTestId("finish-note")).toHaveTextContent("The model declined to answer this one.");
  });

  it("no_note_when_complete", () => {
    renderWithView(<AnswerPanel />, { entry: makeEntry({ finish: "complete" }) });
    expect(screen.queryByTestId("finish-note")).toBeNull();
  });

  it("copy_copies_markdown_source_and_shows_feedback", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    renderWithView(<AnswerPanel />, { entry: makeEntry({ answer: "- **one**\n- two" }) });
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith("- **one**\n- two");
  });

  it("copy_fallback_when_clipboard_missing", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    const exec = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: exec });
    renderWithView(<AnswerPanel />, { entry: makeEntry() });
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(exec).toHaveBeenCalledWith("copy");
  });

  it("answer_region_aria_busy_while_streaming", () => {
    const h = renderWithView(<AnswerPanel />, { phase: "answering", entry: makeEntry({ streaming: true, finish: null }) });
    const region = screen.getByTestId("answer-scroll");
    expect(region).toHaveAttribute("aria-live", "polite");
    expect(region).toHaveAttribute("aria-busy", "true");
    h.update({ entry: makeEntry({ streaming: false }) });
    expect(region).toHaveAttribute("aria-busy", "false");
  });

  it("auto_scroll_sticks_then_stops_on_user_scroll_up", () => {
    const streamingView = (answer: string) => ({ phase: "answering" as const, entry: makeEntry({ streaming: true, finish: null, answer }) });
    const h = renderWithView(<AnswerPanel />, streamingView("one"));
    const el = screen.getByTestId("answer-scroll");
    setScrollGeometry(el, { scrollHeight: 1000, clientHeight: 100 });
    h.update(streamingView("one two"));
    expect(el.scrollTop).toBe(1000);

    // user scrolls up
    el.scrollTop = 300;
    fireEvent.scroll(el);
    setScrollGeometry(el, { scrollHeight: 1200, clientHeight: 100 });
    h.update(streamingView("one two three"));
    expect(el.scrollTop).toBe(300);

    // user scrolls back to the bottom → resumes
    el.scrollTop = 1100;
    fireEvent.scroll(el);
    setScrollGeometry(el, { scrollHeight: 1400, clientHeight: 100 });
    h.update(streamingView("one two three four"));
    expect(el.scrollTop).toBe(1400);
  });

  it("report_first_paint_once_per_stream", () => {
    const h = renderWithView(<AnswerPanel />, { phase: "answering", entry: makeEntry({ streaming: true, answer: "", finish: null }) });
    expect(h.actions.reportFirstPaint).not.toHaveBeenCalled();
    h.update({ phase: "answering", entry: makeEntry({ streaming: true, answer: "I", finish: null }) });
    expect(h.actions.reportFirstPaint).toHaveBeenCalledTimes(1);
    h.update({ phase: "answering", entry: makeEntry({ streaming: true, answer: "I have", finish: null }) });
    h.update({ phase: "idle", entry: makeEntry({ streaming: false, answer: "I have done it" }) });
    expect(h.actions.reportFirstPaint).toHaveBeenCalledTimes(1);
    // next session
    h.update({ phase: "answering", entry: makeEntry({ streaming: true, answer: "", finish: null }) });
    h.update({ phase: "answering", entry: makeEntry({ streaming: true, answer: "Sure", finish: null }) });
    expect(h.actions.reportFirstPaint).toHaveBeenCalledTimes(2);
  });

  it("no_first_paint_for_finished_history_entry", () => {
    const h = renderWithView(<AnswerPanel />, { entry: makeEntry({ streaming: false }) });
    expect(h.actions.reportFirstPaint).not.toHaveBeenCalled();
  });

  it("session_error_alert_with_settings_button", async () => {
    const h = renderWithView(<AnswerPanel />, { sessionError: { code: "no_llm_key", message: "Add your Anthropic API key in Settings." } });
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Add your Anthropic API key in Settings.");
    await userEvent.click(within(alert).getByRole("button", { name: "Open Settings" }));
    expect(h.actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it("session_error_without_settings_button_for_other_codes", () => {
    renderWithView(<AnswerPanel />, { sessionError: { code: "no_speech", message: "No speech was heard." } });
    const alert = screen.getByRole("alert");
    expect(within(alert).queryByRole("button")).toBeNull();
  });

  it("session_error_llm_auth_and_no_stt_key_offer_settings", () => {
    for (const code of ["llm_auth", "no_stt_key"] as const) {
      const h = renderWithView(<AnswerPanel />, { sessionError: { code, message: "m" } });
      expect(within(screen.getByRole("alert")).getByRole("button", { name: "Open Settings" })).toBeInTheDocument();
      h.unmount();
    }
  });
});

describe("QuestionPanel", () => {
  it("question_shows_live_transcript_while_live", () => {
    renderWithView(<QuestionPanel />, { phase: "recording", liveTranscript: "What is a closure", entry: makeEntry({ question: "old" }) });
    expect(screen.getByTestId("question-text")).toHaveTextContent("What is a closure");
  });

  it("question_shows_entry_question_when_idle", () => {
    renderWithView(<QuestionPanel />, { phase: "idle", liveTranscript: "", entry: makeEntry({ question: "Why us?" }) });
    expect(screen.getByTestId("question-text")).toHaveTextContent("Why us?");
  });
});

describe("AskBox", () => {
  it("ask_enter_submits_trimmed_and_clears", async () => {
    const h = renderWithView(<AskBox />);
    const input = screen.getByPlaceholderText("Type a question instead…");
    await userEvent.type(input, "   What is Rust?  {Enter}");
    expect(h.actions.ask).toHaveBeenCalledWith("What is Rust?");
    expect(input).toHaveValue("");
  });

  it("ask_button_submits", async () => {
    const h = renderWithView(<AskBox />);
    await userEvent.type(screen.getByPlaceholderText("Type a question instead…"), "Hi");
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    expect(h.actions.ask).toHaveBeenCalledWith("Hi");
  });

  it("ask_blank_does_nothing", async () => {
    const h = renderWithView(<AskBox />);
    await userEvent.type(screen.getByPlaceholderText("Type a question instead…"), "    {Enter}");
    expect(h.actions.ask).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Ask" })).toBeDisabled();
  });

  it("ask_disabled_unless_canAsk", async () => {
    const h = renderWithView(<AskBox />, { canAsk: false });
    await userEvent.type(screen.getByPlaceholderText("Type a question instead…"), "Hello{Enter}");
    expect(h.actions.ask).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Ask" })).toBeDisabled();
  });
});

describe("ProfileControls", () => {
  it("profile_dropdown_hidden_with_one_profile", () => {
    renderWithView(<ProfileControls />);
    expect(screen.queryByLabelText("Profile")).toBeNull();
    expect(screen.getByLabelText("Call type")).toBeInTheDocument();
  });

  it("profile_dropdown_shown_with_two_profiles", async () => {
    const base = makeSettings();
    const settings = makeSettings({
      profiles: [...base.profiles, { ...base.profiles[0]!, id: "p-2", name: "Sales" }],
    });
    const h = renderWithView(<ProfileControls />, { settings, activeProfile: settings.profiles[0]! });
    await userEvent.selectOptions(screen.getByLabelText("Profile"), "p-2");
    expect(h.actions.setActiveProfile).toHaveBeenCalledWith("p-2");
  });

  it("call_type_change_calls_setCallType", async () => {
    const h = renderWithView(<ProfileControls />);
    const select = screen.getByLabelText("Call type");
    expect(within(select).getAllByRole("option")).toHaveLength(6);
    await userEvent.selectOptions(select, "Sales or customer call");
    expect(h.actions.setCallType).toHaveBeenCalledWith("sales");
  });
});

describe("NoticeBar", () => {
  it("notice_dismissable", async () => {
    const h = renderWithView(<NoticeBar />, { notice: "Output device changed — now capturing Speakers." });
    expect(screen.getByTestId("notice")).toHaveTextContent("Output device changed");
    await userEvent.click(screen.getByRole("button", { name: "Dismiss notice" }));
    expect(h.actions.dismissNotice).toHaveBeenCalledTimes(1);
  });

  it("notice_absent_when_null", () => {
    renderWithView(<NoticeBar />);
    expect(screen.queryByTestId("notice")).toBeNull();
  });
});

describe("FullLayout", () => {
  it("full_layout_answer_panel_comes_before_question_and_controls", () => {
    renderWithView(<FullLayout />, { entry: makeEntry() });
    const answer = screen.getByRole("heading", { name: "Suggested answer" });
    const question = screen.getByRole("heading", { name: "Question heard" });
    const record = screen.getByTestId("record-button");
    expect(answer.compareDocumentPosition(question) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(question.compareDocumentPosition(record) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("full_layout_hotkey_hint", () => {
    renderWithView(<FullLayout />, { hotkeyHint: "Shortcut unavailable — another app is using Ctrl+Shift+Space" });
    expect(screen.getByTestId("hotkey-hint")).toHaveTextContent("Shortcut unavailable");
  });

  it("full_layout_core_starting_disables_controls", () => {
    renderWithView(<FullLayout />, { core: "starting", canRecord: false, canAsk: false });
    expect(screen.getByTestId("core-starting")).toHaveTextContent("Starting…");
    expect(screen.getByTestId("record-button")).toBeDisabled();
    expect(screen.getByPlaceholderText("Type a question instead…")).toBeDisabled();
  });

  it("full_layout_core_failed_alert", () => {
    renderWithView(<FullLayout />, { core: "failed", coreError: { code: "internal", message: "The audio engine failed to start." } });
    expect(screen.getByTestId("core-failed")).toHaveAttribute("role", "alert");
    expect(screen.getByTestId("core-failed")).toHaveTextContent("The audio engine failed to start.");
  });

  it("full_layout_recording_shows_meter_and_silence", () => {
    renderWithView(<FullLayout />, {
      phase: "recording",
      canStop: true,
      canRecord: false,
      recording: makeRecording({ silent: true }),
    });
    expect(screen.getByRole("meter")).toBeInTheDocument();
    expect(screen.getByTestId("silence-note")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop & Answer" })).toBeEnabled();
  });

  it("full_layout_countdown_only_in_last_30s", () => {
    vi.spyOn(Date, "now").mockReturnValue(5_000_000);
    const h = renderWithView(<FullLayout />, { phase: "recording", canStop: true, recording: makeRecording({ deadlineMs: 5_045_000 }) });
    expect(screen.queryByTestId("countdown")).toBeNull();
    h.update({ phase: "recording", canStop: true, recording: makeRecording({ deadlineMs: 5_029_000 }) });
    expect(screen.getByTestId("countdown")).toHaveTextContent("Auto-stop in 29 s");
  });
});

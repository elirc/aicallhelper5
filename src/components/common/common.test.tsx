import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ProtectionBadge } from "../ProtectionBadge";
import { makeEntry, makeRecording, mocked, renderWithView } from "../testUtils";
import { copyText } from "./clipboard";
import { countdownSeconds, formatClock } from "./format";
import { HistoryNav } from "./HistoryNav";
import { CALL_TYPES, callTypeLabel, profileFieldLabels } from "./labels";
import { RecordButton } from "./RecordButton";
import { Countdown, LevelMeter, RecordingTimer, SilenceNote } from "./RecordingInfo";
import { dotTone, StatusDot } from "./StatusDot";
import { StyleChips } from "./StyleChips";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("format", () => {
  it("formatClock_renders_m_ss", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(65_400)).toBe("1:05");
    expect(formatClock(-5)).toBe("0:00");
  });

  it("countdownSeconds_only_in_last_30s", () => {
    expect(countdownSeconds(100_000, 60_000)).toBeNull();
    expect(countdownSeconds(100_000, 70_000)).toBe(30);
    expect(countdownSeconds(100_000, 99_001)).toBe(1);
    expect(countdownSeconds(100_000, 100_000)).toBeNull();
  });
});

describe("labels", () => {
  it("call_type_labels_match_spec", () => {
    expect(CALL_TYPES.map((c) => c.label)).toEqual([
      "Behavioral interview",
      "Technical screen",
      "System design",
      "Recruiter screen",
      "Sales or customer call",
      "General meeting",
    ]);
    expect(callTypeLabel("system_design")).toBe("System design");
  });

  it("profile_field_labels_switch_for_sales_and_meeting", () => {
    expect(profileFieldLabels("behavioral")).toEqual({ resume: "Resume", jobDescription: "Job description" });
    expect(profileFieldLabels("sales")).toEqual({ resume: "Background", jobDescription: "Call context" });
    expect(profileFieldLabels("meeting")).toEqual({ resume: "Background", jobDescription: "Call context" });
  });
});

describe("ProtectionBadge", () => {
  it("protection_badge_protected_is_calm", () => {
    renderWithView(<ProtectionBadge />, { protection: "protected" });
    const b = screen.getByTestId("protection-badge");
    expect(b).toHaveTextContent("Hidden from screen capture");
    expect(b).not.toHaveAttribute("role", "alert");
  });

  it("protection_badge_unknown_never_claims_protection", () => {
    renderWithView(<ProtectionBadge />, { protection: "unknown" });
    const b = screen.getByTestId("protection-badge");
    expect(b).toHaveTextContent("Screen-share protection not confirmed yet");
    expect(b).not.toHaveTextContent("Hidden from screen capture");
  });

  it("protection_badge_unprotected_is_alert", () => {
    renderWithView(<ProtectionBadge />, { protection: "unprotected" });
    const b = screen.getByRole("alert");
    expect(b).toHaveTextContent("Windows would not hide this window");
    expect(b).not.toHaveTextContent("Hidden from screen capture");
  });
});

describe("RecordButton", () => {
  it("record_button_idle_records", async () => {
    const h = renderWithView(<RecordButton />, { phase: "idle", canRecord: true });
    const btn = screen.getByRole("button", { name: "Record" });
    await userEvent.click(btn);
    expect(h.actions.record).toHaveBeenCalledTimes(1);
  });

  it("record_button_disabled_unless_canRecord", () => {
    renderWithView(<RecordButton />, { phase: "idle", canRecord: false });
    expect(screen.getByRole("button", { name: "Record" })).toBeDisabled();
  });

  it("record_button_disabled_when_core_not_ready", () => {
    renderWithView(<RecordButton />, { core: "starting", canRecord: true });
    expect(screen.getByRole("button", { name: "Record" })).toBeDisabled();
  });

  it("record_button_recording_stops", async () => {
    const h = renderWithView(<RecordButton />, { phase: "recording", canStop: true, canRecord: false });
    await userEvent.click(screen.getByRole("button", { name: "Stop & Answer" }));
    expect(h.actions.stop).toHaveBeenCalledTimes(1);
    expect(h.actions.record).not.toHaveBeenCalled();
  });

  it("record_button_starting_shows_stop", () => {
    renderWithView(<RecordButton />, { phase: "starting", canStop: true });
    expect(screen.getByRole("button", { name: "Stop & Answer" })).toBeEnabled();
  });

  it("record_button_finalizing_aria_disabled_keeps_focus", async () => {
    const h = renderWithView(<RecordButton />, { phase: "recording", canStop: true });
    const btn = screen.getByRole("button", { name: "Stop & Answer" });
    btn.focus();
    expect(btn).toHaveFocus();
    h.update({ phase: "finalizing", canStop: false, canRecord: false });
    const fin = screen.getByRole("button", { name: "Finalizing…" });
    expect(fin).toBe(btn);
    expect(fin).toHaveAttribute("aria-disabled", "true");
    expect(fin).not.toHaveAttribute("disabled");
    expect(fin).toHaveFocus();
    await userEvent.click(fin);
    fireEvent.click(fin);
    expect(h.actions.stop).not.toHaveBeenCalled();
    expect(h.actions.record).not.toHaveBeenCalled();
  });

  it("record_button_answering_allows_new_record", async () => {
    const h = renderWithView(<RecordButton />, { phase: "answering", canRecord: true });
    await userEvent.click(screen.getByRole("button", { name: "Record" }));
    expect(h.actions.record).toHaveBeenCalledTimes(1);
  });
});

describe("StyleChips", () => {
  it("style_chips_radio_semantics", () => {
    renderWithView(<StyleChips />, { answerStyle: "balanced" });
    expect(screen.getByRole("radiogroup", { name: "Answer style" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Balanced" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Brief" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("radio", { name: "Detailed" })).toHaveAttribute("aria-checked", "false");
  });

  it("style_chip_click_sets_style", async () => {
    const h = renderWithView(<StyleChips />, { answerStyle: "balanced" });
    await userEvent.click(screen.getByRole("radio", { name: "Detailed" }));
    expect(h.actions.setStyle).toHaveBeenCalledWith("detailed");
  });

  it("style_chips_arrow_keys_move_selection", async () => {
    const h = renderWithView(<StyleChips />, { answerStyle: "balanced" });
    screen.getByRole("radio", { name: "Balanced" }).focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(h.actions.setStyle).toHaveBeenLastCalledWith("detailed");
    await userEvent.keyboard("{ArrowLeft}");
    expect(h.actions.setStyle).toHaveBeenLastCalledWith("brief");
  });
});

describe("recording info", () => {
  it("level_meter_width_follows_level", () => {
    renderWithView(<LevelMeter />, { phase: "recording", recording: makeRecording({ level: 0.42 }) });
    const m = screen.getByRole("meter", { name: "Call audio level" });
    expect(m).toHaveAttribute("aria-valuenow", "42");
    expect((m.firstChild as HTMLElement).style.width).toBe("42%");
  });

  it("level_meter_hidden_when_not_recording", () => {
    renderWithView(<LevelMeter />, { recording: null });
    expect(screen.queryByRole("meter")).toBeNull();
  });

  it("countdown_hidden_before_last_30s", () => {
    renderWithView(<Countdown />, { recording: makeRecording({ deadlineMs: Date.now() + 60_000 }) });
    expect(screen.queryByTestId("countdown")).toBeNull();
  });

  it("countdown_shown_in_last_30s", () => {
    vi.spyOn(Date, "now").mockReturnValue(1_000_000);
    renderWithView(<Countdown />, { recording: makeRecording({ deadlineMs: 1_012_000 }) });
    expect(screen.getByTestId("countdown")).toHaveTextContent("Auto-stop in 12 s");
  });

  it("silence_note_only_when_silent", () => {
    const h = renderWithView(<SilenceNote />, { recording: makeRecording({ silent: false }) });
    expect(screen.queryByTestId("silence-note")).toBeNull();
    h.update({ recording: makeRecording({ silent: true }) });
    expect(screen.getByTestId("silence-note")).toHaveTextContent(
      "No call audio detected yet — make sure the call is playing through your default output device.",
    );
  });

  it("recording_timer_shows_elapsed", () => {
    vi.spyOn(Date, "now").mockReturnValue(2_000_000);
    renderWithView(<RecordingTimer />, { recording: makeRecording({ startedAtMs: 2_000_000 - 75_000 }) });
    expect(screen.getByTestId("rec-timer")).toHaveTextContent("1:15");
  });
});

describe("HistoryNav", () => {
  it("history_hidden_without_entries", () => {
    renderWithView(<HistoryNav />);
    expect(screen.queryByRole("navigation")).toBeNull();
  });

  it("history_arrows_position_tag_and_clear", async () => {
    const h = renderWithView(<HistoryNav />, {
      entry: makeEntry({ callType: "technical" }),
      history: { index: 2, count: 3, canPrev: true, canNext: true },
    });
    expect(screen.getByTestId("history-pos")).toHaveTextContent("2/3");
    expect(screen.getByTestId("history-tag")).toHaveTextContent("Technical screen");
    await userEvent.click(screen.getByRole("button", { name: "Previous answer" }));
    await userEvent.click(screen.getByRole("button", { name: "Next answer" }));
    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(h.actions.historyPrev).toHaveBeenCalledTimes(1);
    expect(h.actions.historyNext).toHaveBeenCalledTimes(1);
    expect(h.actions.clearHistory).toHaveBeenCalledTimes(1);
  });

  it("history_clear_available_from_one_entry", () => {
    renderWithView(<HistoryNav />, { entry: makeEntry(), history: { index: 1, count: 1, canPrev: false, canNext: false } });
    expect(screen.getByRole("button", { name: "Clear" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Previous answer" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next answer" })).toBeDisabled();
  });
});

describe("StatusDot", () => {
  it("status_dot_tone_by_core_and_phase", () => {
    expect(dotTone({ core: "failed", phase: "idle" })).toBe("bad");
    expect(dotTone({ core: "starting", phase: "idle" })).toBe("warn");
    expect(dotTone({ core: "ready", phase: "recording" })).toBe("rec");
    expect(dotTone({ core: "ready", phase: "answering" })).toBe("busy");
    expect(dotTone({ core: "ready", phase: "idle" })).toBe("ok");
    render(<StatusDot view={{ core: "ready", phase: "recording" }} />);
    expect(screen.getByRole("img", { name: "Recording" })).toBeInTheDocument();
  });
});

describe("copyText", () => {
  it("copy_uses_async_clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    await expect(copyText("**hi**")).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith("**hi**");
  });

  it("copy_falls_back_to_textarea_when_clipboard_rejects", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("denied"));
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    let copied = "";
    const exec = vi.fn(() => {
      copied = (document.activeElement as HTMLTextAreaElement).value;
      return true;
    });
    Object.defineProperty(document, "execCommand", { configurable: true, value: exec });
    await act(async () => {
      await expect(copyText("fallback text")).resolves.toBe(true);
    });
    expect(exec).toHaveBeenCalledWith("copy");
    expect(copied).toBe("fallback text");
    expect(document.querySelector("textarea")).toBeNull();
  });

  it("copy_reports_failure_when_everything_fails", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    Object.defineProperty(document, "execCommand", { configurable: true, value: vi.fn(() => false) });
    await expect(copyText("x")).resolves.toBe(false);
    expect(mocked(document.execCommand)).toHaveBeenCalled();
  });
});

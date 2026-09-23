import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { makeSettings } from "../app/testing";
import type { AppView } from "../app/view";
import type { Protection } from "../generated/Protection";
import { Root } from "./Root";
import { makeEntry, makeRecording, renderWithView } from "./testUtils";

vi.mock("../markdown", () => ({
  Markdown: ({ source }: { source: string }) => <div data-testid="md">{source}</div>,
}));

const VIEWS: Record<string, Partial<AppView>> = {
  full: { layout: "full", screen: "main" },
  prompter: { layout: "prompter", screen: "main" },
  settings: { screen: "settings" },
};

describe("Root", () => {
  it("root_picks_full_layout", () => {
    renderWithView(<Root />, VIEWS.full);
    expect(screen.getByTestId("full-layout")).toBeInTheDocument();
  });

  it("root_picks_prompter_layout", () => {
    renderWithView(<Root />, VIEWS.prompter);
    expect(screen.getByTestId("prompter-layout")).toBeInTheDocument();
  });

  it("root_settings_screen_wins_over_layout", () => {
    renderWithView(<Root />, { screen: "settings", layout: "prompter" });
    expect(screen.getByTestId("settings-screen")).toBeInTheDocument();
  });

  const expected: Record<Protection, string> = {
    protected: "Hidden from screen capture",
    unknown: "Screen-share protection not confirmed yet",
    unprotected: "Windows would not hide this window",
  };

  for (const [name, over] of Object.entries(VIEWS)) {
    for (const protection of ["protected", "unknown", "unprotected"] as const) {
      it(`protection_badge_${protection}_in_${name}_view`, () => {
        renderWithView(<Root />, { ...over, protection });
        const badges = screen.getAllByTestId("protection-badge");
        expect(badges).toHaveLength(1);
        expect(badges[0]).toHaveTextContent(expected[protection]);
        if (protection !== "protected") {
          expect(document.body.textContent).not.toContain("Hidden from screen capture");
        }
        if (protection === "unprotected") {
          expect(badges[0]).toHaveAttribute("role", "alert");
        }
      });
    }
  }

  it("no_view_ever_says_microphone", () => {
    const states: Array<Partial<AppView>> = [
      {},
      { phase: "starting", statusText: "Starting system-audio capture…", canStop: true },
      { phase: "recording", canStop: true, recording: makeRecording({ silent: true, deadlineMs: Date.now() + 10_000 }) },
      { phase: "finalizing", statusText: "Finalizing transcript…" },
      { phase: "answering", entry: makeEntry({ streaming: true, finish: null }) },
      { entry: makeEntry({ finish: "truncated" }), history: { index: 1, count: 1, canPrev: false, canNext: false } },
      { needsSetup: true, sessionError: { code: "no_stt_key", message: "Add your Deepgram key in Settings." } },
      { core: "starting" },
      { core: "failed", coreError: { code: "internal", message: "Core failed." } },
      { protection: "unknown", notice: "Output device changed." },
      { protection: "unprotected" },
    ];
    const settingsStates: Array<Partial<AppView>> = [
      {},
      { phase: "recording", canStop: true },
      {
        settings: makeSettings({
          loadIssue: { status: "corrupt", backupPath: "C:\\b.bak", writesBlocked: true, message: "Damaged." },
          hotkeyStatus: "unavailable",
        }),
      },
    ];
    const all: Array<Partial<AppView>> = [
      ...states.map((s) => ({ ...s, ...VIEWS.full })),
      ...states.map((s) => ({ ...s, ...VIEWS.prompter })),
      ...settingsStates.map((s) => ({ ...s, ...VIEWS.settings })),
    ];
    for (const over of all) {
      const h = renderWithView(<Root />, over);
      const html = h.container.innerHTML.toLowerCase();
      expect(html).not.toContain("microphone");
      expect(html).not.toMatch(/\bmic\b/);
      h.unmount();
    }
  });
});

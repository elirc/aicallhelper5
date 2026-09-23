// End-to-end page wiring: the real <App> (AppProvider + the real components)
// against the fake core. Guards the v3 failure mode where each side was only
// ever tested against a mock of the other (spec §14.1).
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { App } from "../App";
import { createFakeApi } from "../ipc/fake";

describe("<App> with the fake core", () => {
  it("record -> events -> stop -> streamed answer lands in the DOM", async () => {
    const api = createFakeApi();
    render(<App api={api} />);
    const status = await screen.findByTestId("status-text");
    await screen.findByText("Ready — press Record while the other person is speaking");

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /^Record/ }));
    expect(api.callsOf("startSession")).toHaveLength(1);
    await screen.findByText("Starting system-audio capture…");

    act(() => {
      api.emit({ type: "session:recording", sessionId: "s1", deadlineMs: Date.now() + 120_000, capMs: 120_000 });
      api.emit({ type: "stt:partial", sessionId: "s1", text: "Why do you want this job?", isFinal: true });
    });
    expect(status.textContent).toMatch(/^Recording — \d:\d\d$/);
    expect(screen.getAllByText("Why do you want this job?").length).toBeGreaterThan(0);

    await user.click(screen.getByRole("button", { name: /Stop & Answer/ }));
    expect(api.callsOf("stopSession")).toEqual([["s1"]]);
    act(() => {
      api.emit({ type: "llm:delta", sessionId: "s1", delta: "Because **the mission** " });
      api.emit({ type: "llm:delta", sessionId: "s1", delta: "matters to me." });
    });
    // Deltas paint on the next animation frame.
    await screen.findByText("the mission");
    act(() => {
      api.emit({
        type: "llm:done",
        sessionId: "s1",
        transcript: "Why do you want this job?",
        answer: "Because **the mission** matters to me.",
        finish: "complete",
        callType: "behavioral",
        metrics: { audioDrainMs: 12, sttFinalizeMs: 150, firstTokenMs: 640, totalMs: 1500 },
      });
    });
    expect(screen.getByText("the mission").tagName).toBe("STRONG");
    await screen.findByText("Ready — press Record while the other person is speaking");
  });
});

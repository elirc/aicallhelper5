import { describe, expect, it } from "vitest";
import { makeSettings } from "../app/testing";
import type { EventEnvelope } from "../generated/EventEnvelope";
import * as copy from "./copy";
import { HOTKEY, STATUS } from "./copy";
import { initialState, reducer } from "./reducer";
import { hotkeyHint, isSilent, needsSetup, selectView } from "./select";
import type { Action, State } from "./types";

const run = (s: State, ...a: Action[]) => a.reduce(reducer, s);
const snap = (core: "starting" | "ready" | "failed" = "ready"): Action => ({
  type: "status/snapshot",
  snapshot: { revision: 1, core, coreError: core === "failed" ? { code: "internal", message: "Core exploded." } : null, protection: "unknown", session: { id: null, phase: "idle", recording: null } },
});
const ready = () => run(initialState(), snap(), { type: "settings/adopt", settings: makeSettings() });
const evs = (s: State, envelopes: EventEnvelope[], now = 0) => reducer(s, { type: "events", envelopes, now });

function recordingAt(startedAt: number): State {
  const s = run(ready(), { type: "cmd/start", gen: 1 }, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s1" } });
  return evs(s, [{ seq: 1, type: "session:recording", sessionId: "s1", deadlineMs: startedAt + 120_000, capMs: 120_000 }]);
}

describe("status text (spec §13)", () => {
  it("core starting / failed", () => {
    expect(selectView(initialState(), 0).statusText).toBe(STATUS.coreStarting);
    expect(selectView(run(initialState(), snap("failed")), 0).statusText).toBe("Core exploded.");
  });

  it("ready vs needsSetup", () => {
    expect(selectView(ready(), 0).statusText).toBe("Ready — press Record while the other person is speaking");
    const noKeys = run(ready(), { type: "settings/adopt", settings: makeSettings({ settingsRevision: 2, keys: [] }) });
    expect(selectView(noKeys, 0).statusText).toBe("Add your API keys — open Settings to get started");
  });

  it("starting / recording elapsed / finalizing / answering", () => {
    const starting = run(ready(), { type: "cmd/start", gen: 1 });
    expect(selectView(starting, 0).statusText).toBe("Starting system-audio capture…");
    const rec = recordingAt(10_000);
    expect(selectView(rec, 22_400).statusText).toBe("Recording — 0:12");
    expect(selectView(rec, 10_000 + 75_000).statusText).toBe("Recording — 1:15");
    const fin = run(rec, { type: "cmd/stop", gen: 2, clickedAtMs: 0 }, { type: "cmd/stop-result", sessionId: "s1", result: { ok: true, value: null } });
    expect(selectView(fin, 0).statusText).toBe("Finalizing transcript…");
    const ans = evs(fin, [{ seq: 2, type: "llm:delta", sessionId: "s1", delta: "x" }]);
    expect(selectView(ans, 0).statusText).toBe("Generating answer…");
  });

  it('no status string or copy constant ever says "microphone"', () => {
    const strings: string[] = [];
    const collect = (v: unknown) => {
      if (typeof v === "string") strings.push(v);
      else if (v && typeof v === "object") Object.values(v).forEach(collect);
    };
    collect(copy);
    strings.push(copy.recordingStatus(12_000));
    // Every reachable status line.
    const states: Array<[State, number]> = [
      [initialState(), 0],
      [run(initialState(), snap("failed")), 0],
      [ready(), 0],
      [run(ready(), { type: "settings/adopt", settings: makeSettings({ settingsRevision: 2, keys: [] }) }), 0],
      [run(ready(), { type: "cmd/start", gen: 1 }), 0],
      [recordingAt(0), 7_000],
      [evs(recordingAt(0), [{ seq: 5, type: "session:autostopped", sessionId: "s1" }]), 0],
      [evs(recordingAt(0), [{ seq: 5, type: "llm:delta", sessionId: "s1", delta: "a" }]), 0],
    ];
    for (const [s, now] of states) {
      const v = selectView(s, now);
      strings.push(v.statusText, v.hotkeyHint);
    }
    expect(strings.length).toBeGreaterThan(15);
    for (const s of strings) expect(s.toLowerCase()).not.toContain("microphone");
  });
});

describe("silence detection", () => {
  it("silent after 5 s with no loud level, measured from start", () => {
    const s = recordingAt(100_000);
    expect(isSilent(s.session, 104_999)).toBe(false);
    expect(isSilent(s.session, 105_000)).toBe(true);
    expect(selectView(s, 105_000).recording?.silent).toBe(true);
  });

  it("a loud level (rms >= 0.01) resets the window; quiet levels do not", () => {
    let s = recordingAt(100_000);
    s = evs(s, [{ seq: 2, type: "audio:level", sessionId: "s1", rms: 0.01 }], 103_000);
    expect(isSilent(s.session, 107_999)).toBe(false);
    expect(isSilent(s.session, 108_000)).toBe(true);
    s = evs(s, [{ seq: 3, type: "audio:level", sessionId: "s1", rms: 0.009 }], 108_500);
    expect(isSilent(s.session, 108_600)).toBe(true);
  });

  it("never silent outside recording", () => {
    expect(isSilent(run(ready(), { type: "cmd/start", gen: 1 }).session, 1e12)).toBe(false);
  });
});

describe("needsSetup", () => {
  it("false until settings load", () => expect(needsSetup(null)).toBe(false));
  it("true without a deepgram key", () => {
    const s = makeSettings();
    expect(needsSetup({ ...s, keys: s.keys.map((k) => (k.id === "deepgram" ? { ...k, hasKey: false } : k)) })).toBe(true);
  });
  it("true when the selected provider's key is missing", () => {
    expect(needsSetup(makeSettings({ llmProvider: "groq" }))).toBe(true);
    expect(needsSetup(makeSettings())).toBe(false);
  });
});

describe("can* flags", () => {
  it("record when core ready and idle or answering (supersedes); stop while starting/recording and not already stopping", () => {
    expect(selectView(initialState(), 0).canRecord).toBe(false);
    expect(selectView(ready(), 0).canRecord).toBe(true);
    const rec = recordingAt(0);
    expect(selectView(rec, 0)).toMatchObject({ canRecord: false, canStop: true });
    const stopping = run(rec, { type: "cmd/stop", gen: 2, clickedAtMs: 0 });
    expect(selectView(stopping, 0).canStop).toBe(false);
    const fin = run(stopping, { type: "cmd/stop-result", sessionId: "s1", result: { ok: true, value: null } });
    expect(selectView(fin, 0)).toMatchObject({ canStop: false, canRecord: false, phase: "finalizing" });
    const ans = evs(fin, [{ seq: 9, type: "llm:delta", sessionId: "s1", delta: "x" }]);
    expect(selectView(ans, 0)).toMatchObject({ canRecord: true, phase: "answering" });
  });

  it("ask needs a ready core; regenerate needs a displayed entry with a question", () => {
    expect(selectView(initialState(), 0).canAsk).toBe(false);
    expect(selectView(ready(), 0)).toMatchObject({ canAsk: true, canRegenerate: false });
  });

  it("aborted errors are not surfaced as a session error", () => {
    let s = run(ready(), { type: "cmd/ask", gen: 1, question: "q", clickedAtMs: 0 }, { type: "cmd/ask-result", gen: 1, result: { ok: true, value: "s1" } });
    s = evs(s, [{ seq: 1, type: "session:error", sessionId: "s1", error: { code: "aborted", message: "Cancelled." } }]);
    expect(selectView(s, 0).sessionError).toBeNull();
  });
});

describe("derived settings fields", () => {
  it("provider name, style, layout, fonts, active profile", () => {
    const v = selectView(run(ready(), { type: "settings/adopt", settings: makeSettings({ settingsRevision: 2, llmProvider: "groq", answerStyle: "brief", layoutMode: "prompter", answerFontPx: 16, prompterFontPx: 24 }) }), 0);
    expect(v).toMatchObject({ providerName: "Groq GPT-OSS 120B (fastest)", answerStyle: "brief", layout: "prompter", answerFontPx: 16, prompterFontPx: 24 });
    expect(v.activeProfile?.id).toBe("default");
  });

  it("hotkey hint reflects the honest registration status", () => {
    expect(hotkeyHint(makeSettings())).toBe("Ctrl+Shift+Space toggles recording from any app");
    expect(hotkeyHint(makeSettings({ hotkeyStatus: "disabled", hotkeyRegistered: false }))).toBe(HOTKEY.disabled);
    expect(hotkeyHint(makeSettings({ hotkeyStatus: "unavailable", hotkeyMessage: "Taken by X" }))).toBe("Taken by X");
    expect(hotkeyHint(makeSettings({ hotkeyStatus: "invalid", hotkeyMessage: null }))).toBe(HOTKEY.invalid);
  });
});

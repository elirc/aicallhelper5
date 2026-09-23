import { StrictMode } from "react";
import { act, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createFakeApi, STOP_NOT_TAKEN, type FakeApi, type FakeApiOptions } from "../ipc/fake";
import type { CoreEvent } from "../generated/CoreEvent";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import { NOTICE, STATUS } from "../state/copy";
import { AppProvider } from "./AppProvider";
import { makeSettings } from "./testing";
import { useApp, type AppContextValue } from "./view";

let current: AppContextValue;
let renders = 0;
function Probe() {
  current = useApp();
  renders++;
  return null;
}

interface Setup {
  api: FakeApi;
  frames: Array<() => void>;
  runFrame(): void;
  clock: { t: number; p: number };
  copied: string[];
  unmount(): void;
}

function setup(opts: FakeApiOptions & { strict?: boolean; copyOk?: boolean } = {}): Setup {
  const api = createFakeApi(opts);
  const frames: Array<() => void> = [];
  const clock = { t: 1_000_000, p: 0 };
  const copied: string[] = [];
  const scheduleFrame = (cb: () => void) => {
    frames.push(cb);
    return () => {
      const i = frames.indexOf(cb);
      if (i >= 0) frames.splice(i, 1);
    };
  };
  const tree = (
    <AppProvider
      api={api}
      scheduleFrame={scheduleFrame}
      clock={{ now: () => clock.t, perf: () => clock.p }}
      copyText={async (t) => {
        copied.push(t);
        return opts.copyOk ?? true;
      }}
    >
      <Probe />
    </AppProvider>
  );
  const r = render(opts.strict ? <StrictMode>{tree}</StrictMode> : tree);
  return {
    api,
    frames,
    clock,
    copied,
    runFrame() {
      act(() => {
        const cbs = frames.splice(0);
        cbs.forEach((cb) => cb());
      });
    },
    unmount: r.unmount,
  };
}

/** Let fake-api promises and React updates settle (no timers involved). */
async function settle() {
  await act(async () => {
    for (let i = 0; i < 20; i++) await Promise.resolve();
  });
}
async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
function emit(api: FakeApi, ev: CoreEvent) {
  act(() => {
    api.emit(ev);
  });
}
async function startRecording(s: Setup): Promise<string> {
  act(() => current.actions.record());
  await settle();
  const id = s.api.core.status.session.id as string;
  emit(s.api, { type: "session:recording", sessionId: id, deadlineMs: s.clock.t + 120_000, capMs: 120_000 });
  expect(current.view.phase).toBe("recording");
  return id;
}

afterEach(() => {
  vi.useRealTimers();
});

describe("mount wiring", () => {
  it("subscribes to events BEFORE calling get_status / get_settings (§14.1)", async () => {
    const s = setup();
    await settle();
    const methods = s.api.calls.map((c) => c.method);
    expect(methods[0]).toBe("subscribe");
    expect(methods.slice(1, 3).sort()).toEqual(["getSettings", "getStatus"]);
    expect(current.view.core).toBe("ready");
    expect(current.view.settings?.settingsRevision).toBe(1);
    expect(current.view.statusText).toBe(STATUS.ready);
  });

  it("re-adopts a live session from get_status on load, keeping its deadline", async () => {
    const status: Partial<StatusSnapshot> = {
      revision: 7,
      session: { id: "s5", phase: "recording", recording: { deadlineMs: 1_060_000, capMs: 120_000 } },
    };
    const s = setup({ status });
    await settle();
    expect(current.view.phase).toBe("recording");
    expect(current.view.recording).toMatchObject({ deadlineMs: 1_060_000, capMs: 120_000, startedAtMs: 940_000 });
    expect(current.view.canStop).toBe(true);
    // Events for the adopted session land; Stop targets it.
    emit(s.api, { type: "stt:partial", sessionId: "s5", text: "still here", isFinal: false });
    expect(current.view.liveTranscript).toBe("still here");
    act(() => current.actions.stop());
    await settle();
    expect(s.api.callsOf("stopSession")).toEqual([["s5"]]);
    expect(current.view.phase).toBe("finalizing");
  });

  it("StrictMode double-mount: one initial fetch, every event applied exactly once", async () => {
    const s = setup({ strict: true });
    await settle();
    expect(s.api.callsOf("getStatus")).toHaveLength(1);
    expect(s.api.listenerCount()).toBe(1);
    await startRecording(s);
    act(() => current.actions.stop());
    await settle();
    emit(s.api, { type: "llm:delta", sessionId: "s1", delta: "one " });
    s.runFrame();
    emit(s.api, { type: "llm:delta", sessionId: "s1", delta: "two" });
    s.runFrame();
    expect(current.view.entry?.answer).toBe("one two");
  });

  it("core:ready refetches settings + status; settings:changed refetches settings", async () => {
    const s = setup();
    await settle();
    const before = { st: s.api.callsOf("getStatus").length, se: s.api.callsOf("getSettings").length };
    emit(s.api, { type: "core:ready", revision: 9 });
    await settle();
    expect(s.api.callsOf("getStatus").length).toBe(before.st + 1);
    expect(s.api.callsOf("getSettings").length).toBe(before.se + 1);
    s.api.core.settings = makeSettings({ settingsRevision: 5, answerStyle: "detailed" });
    emit(s.api, { type: "settings:changed" });
    await settle();
    expect(current.view.answerStyle).toBe("detailed");
  });

  it("duplicate envelopes (seq <= last) are ignored, including side effects", async () => {
    const s = setup();
    await settle();
    act(() => s.api.emitEnvelope({ seq: 10, type: "hotkey:toggle" }));
    await settle();
    act(() => s.api.emitEnvelope({ seq: 10, type: "hotkey:toggle" }));
    act(() => s.api.emitEnvelope({ seq: 3, type: "hotkey:toggle" }));
    await settle();
    expect(s.api.callsOf("startSession")).toHaveLength(1);
  });
});

describe("delta coalescing", () => {
  it("buffers llm:delta and dispatches at most once per animation frame", async () => {
    const s = setup();
    await settle();
    await startRecording(s);
    act(() => current.actions.stop());
    await settle();
    const r0 = renders;
    act(() => {
      s.api.emit({ type: "llm:delta", sessionId: "s1", delta: "a" });
      s.api.emit({ type: "llm:delta", sessionId: "s1", delta: "b" });
      s.api.emit({ type: "llm:delta", sessionId: "s1", delta: "c" });
    });
    expect(renders).toBe(r0);
    expect(current.view.entry?.answer).toBe("");
    expect(s.frames).toHaveLength(1);
    s.runFrame();
    expect(current.view.entry?.answer).toBe("abc");
    expect(renders).toBe(r0 + 1);
  });

  it("a non-delta event flushes buffered deltas first, preserving order", async () => {
    const s = setup();
    await settle();
    await startRecording(s);
    act(() => current.actions.stop());
    await settle();
    act(() => {
      s.api.emit({ type: "llm:delta", sessionId: "s1", delta: "partial" });
      s.api.emit({
        type: "llm:done",
        sessionId: "s1",
        transcript: "q",
        answer: "partial answer",
        finish: "complete",
        callType: "behavioral",
        metrics: { audioDrainMs: 1, sttFinalizeMs: 2, firstTokenMs: 3, totalMs: 4 },
      });
    });
    expect(s.frames).toHaveLength(0);
    expect(current.view.phase).toBe("idle");
    expect(current.view.entry?.answer).toBe("partial answer");
    expect(current.view.history.count).toBe(1);
  });
});

describe("hotkey + window", () => {
  it("hotkey toggles record/stop on the main screen", async () => {
    const s = setup();
    await settle();
    emit(s.api, { type: "hotkey:toggle" });
    await settle();
    expect(s.api.callsOf("startSession")).toHaveLength(1);
    emit(s.api, { type: "hotkey:toggle" });
    await settle();
    expect(s.api.callsOf("stopSession")).toEqual([["s1"]]);
  });

  it("while Settings is open the hotkey may only STOP, never start", async () => {
    const s = setup();
    await settle();
    act(() => current.actions.openSettings());
    emit(s.api, { type: "hotkey:toggle" });
    await settle();
    expect(s.api.callsOf("startSession")).toHaveLength(0);
    act(() => current.actions.closeSettings());
    await startRecording(s);
    act(() => current.actions.openSettings());
    emit(s.api, { type: "hotkey:toggle" });
    await settle();
    expect(s.api.callsOf("stopSession")).toEqual([["s1"]]);
    expect(current.view.screen).toBe("settings");
  });

  it("window:close-requested sets closeRequested; the close guard follows settingsDirty", async () => {
    const s = setup();
    await settle();
    act(() => current.actions.setSettingsDirty(true));
    await settle();
    expect(s.api.callsOf("setCloseGuard")).toEqual([[true]]);
    emit(s.api, { type: "window:close-requested" });
    expect(current.view.closeRequested).toBe(true);
    act(() => current.actions.dismissCloseRequest());
    expect(current.view.closeRequested).toBe(false);
    act(() => current.actions.setSettingsDirty(false));
    await settle();
    expect(s.api.callsOf("setCloseGuard")).toEqual([[true], [false]]);
  });
});

describe("commands", () => {
  it("Stop clicked while start is still in flight is sent once the id arrives", async () => {
    const s = setup();
    await settle();
    let release!: () => void;
    s.api.respond("startSession", () => new Promise((res) => (release = () => res({ ok: true, value: "s1" }))));
    act(() => current.actions.record());
    act(() => current.actions.stop());
    expect(current.view.canStop).toBe(false);
    expect(s.api.callsOf("stopSession")).toHaveLength(0);
    release();
    await settle();
    expect(s.api.callsOf("stopSession")).toEqual([["s1"]]);
  });

  it("a start that resolves after Cancel tears down only the session it created", async () => {
    const s = setup();
    await settle();
    let release!: () => void;
    s.api.respond("startSession", () => new Promise((res) => (release = () => res({ ok: true, value: "s1" }))));
    act(() => current.actions.record());
    act(() => current.actions.cancel());
    release();
    await settle();
    expect(s.api.callsOf("cancelSession")).toEqual([["s1"]]);
    expect(current.view.phase).toBe("idle");
  });

  it("a late start result after a newer ask does not touch the newer session", async () => {
    const s = setup();
    await settle();
    let release!: () => void;
    s.api.respond("startSession", () => new Promise((res) => (release = () => res({ ok: true, value: "s1" }))));
    s.api.respond("ask", () => ({ ok: true, value: "s2" }));
    act(() => current.actions.record());
    act(() => current.actions.ask("typed"));
    await settle();
    release();
    await settle();
    expect(s.api.callsOf("cancelSession")).toEqual([["s1"]]);
    emit(s.api, { type: "llm:delta", sessionId: "s2", delta: "yes" });
    s.runFrame();
    expect(current.view.entry?.answer).toBe("yes");
    expect(current.view.phase).toBe("answering");
  });

  it("ask trims, rejects empty with a notice, and measures visible first word", async () => {
    const s = setup();
    await settle();
    act(() => current.actions.ask("   "));
    expect(current.view.notice).toBe(NOTICE.emptyQuestion);
    expect(s.api.callsOf("ask")).toHaveLength(0);
    s.clock.p = 100;
    act(() => current.actions.ask("  What is a monad?  "));
    await settle();
    expect(s.api.callsOf("ask")).toEqual([["What is a monad?"]]);
    emit(s.api, { type: "llm:delta", sessionId: "s1", delta: "A" });
    s.runFrame();
    s.clock.p = 480;
    act(() => current.actions.reportFirstPaint());
    s.clock.p = 900;
    act(() => current.actions.reportFirstPaint());
    expect(current.view.entry?.visibleFirstWordMs).toBe(380);
  });

  it("regenerate asks with the viewed entry's question and adds a new entry", async () => {
    const s = setup();
    await settle();
    for (const [q, a] of [["Q1", "A1"], ["Q2", "A2"]] as const) {
      act(() => current.actions.ask(q));
      await settle();
      const id = s.api.core.status.session.id as string;
      emit(s.api, { type: "llm:done", sessionId: id, transcript: q, answer: a, finish: "complete", callType: "behavioral", metrics: { audioDrainMs: 0, sttFinalizeMs: 0, firstTokenMs: 1, totalMs: 2 } });
      s.api.core.status = { ...s.api.core.status, session: { id: null, phase: "idle", recording: null } };
    }
    act(() => current.actions.historyPrev());
    expect(current.view.entry?.question).toBe("Q1");
    act(() => current.actions.regenerate());
    await settle();
    expect(s.api.callsOf("ask").at(-1)).toEqual(["Q1"]);
    expect(current.view.history).toMatchObject({ index: 3, count: 3 });
  });

  it("events emitted by the core BEFORE the start/ask reply are not lost (fake core emits during the call)", async () => {
    const s = setup();
    await settle();
    s.api.respond("ask", () => {
      s.api.emit({ type: "stt:partial", sessionId: "s7", text: "Typed?", isFinal: true });
      return { ok: true, value: "s7" };
    });
    act(() => current.actions.ask("Typed?"));
    await settle();
    expect(current.view.liveTranscript).toBe("Typed?");
    s.api.respond("startSession", () => {
      s.api.emit({ type: "session:recording", sessionId: "s8", deadlineMs: s.clock.t + 120_000, capMs: 120_000 });
      return { ok: true, value: "s8" };
    });
    act(() => current.actions.record());
    await settle();
    expect(current.view.phase).toBe("recording");
    expect(current.view.recording?.deadlineMs).toBe(s.clock.t + 120_000);
  });

  it("cancel calls cancel_session for the live session", async () => {
    const s = setup();
    await settle();
    await startRecording(s);
    act(() => current.actions.cancel());
    await settle();
    expect(s.api.callsOf("cancelSession")).toEqual([["s1"]]);
    expect(current.view.phase).toBe("idle");
  });
});

describe("settings", () => {
  it("stale save: re-fetches settings, reports stale:true, and the next save uses the fresh revision", async () => {
    const s = setup();
    await settle();
    // Someone else saved meanwhile (revision 1 -> 2).
    s.api.core.settings = makeSettings({ settingsRevision: 2 });
    let r!: Awaited<ReturnType<AppContextValue["actions"]["saveSettings"]>>;
    await act(async () => {
      r = await current.actions.saveSettings({ hotkey: "Ctrl+Alt+K" });
    });
    expect(r).toEqual({ ok: false, message: NOTICE.staleSave, stale: true });
    expect(current.view.settings?.settingsRevision).toBe(2);
    await act(async () => {
      r = await current.actions.saveSettings({ hotkey: "Ctrl+Alt+K" });
    });
    expect(r).toEqual({ ok: true });
    expect(s.api.callsOf("setSettings").map((a) => (a[0] as { baseRevision: number }).baseRevision)).toEqual([1, 2]);
    expect(current.view.settings?.hotkey).toBe("Ctrl+Alt+K");
  });

  it("a non-stale failure reports the core's message with stale:false", async () => {
    const s = setup();
    await settle();
    s.api.respond("setSettings", () => ({ ok: false, error: { code: "internal", message: "Encryption failed; nothing saved." } }));
    let r!: Awaited<ReturnType<AppContextValue["actions"]["saveSettings"]>>;
    await act(async () => {
      r = await current.actions.saveSettings({ secrets: [{ keyId: "deepgram", action: "set", value: "k" }] });
    });
    expect(r).toEqual({ ok: false, message: "Encryption failed; nothing saved.", stale: false });
  });

  it("quick saves are optimistic and serialized (rapid A+ A+ both land)", async () => {
    const s = setup();
    await settle();
    act(() => {
      current.actions.bumpAnswerFont(1);
      current.actions.bumpAnswerFont(1);
    });
    expect(current.view.answerFontPx).toBe(18);
    await settle();
    expect(s.api.callsOf("setSettings").map((a) => a[0])).toEqual([
      { answerFontPx: 16, baseRevision: 1 },
      { answerFontPx: 18, baseRevision: 2 },
    ]);
    expect(current.view.notice).toBeNull();
    expect(current.view.settings?.settingsRevision).toBe(3);
  });

  it("font bumps clamp to the allowed ranges", async () => {
    const s = setup({ settings: makeSettings({ answerFontPx: 22, prompterFontPx: 14 }) });
    await settle();
    act(() => {
      current.actions.bumpAnswerFont(1);
      current.actions.bumpPrompterFont(-1);
    });
    await settle();
    expect(s.api.callsOf("setSettings")).toHaveLength(0);
  });

  it("setCallType patches the active profile; setStyle/setLayout/setActiveProfile save", async () => {
    const two = makeSettings({
      profiles: [
        { id: "default", name: "Default", callType: "behavioral", focus: "", resume: "", jobDescription: "", notes: "" },
        { id: "p2", name: "Sales", callType: "sales", focus: "", resume: "", jobDescription: "", notes: "" },
      ],
    });
    const s = setup({ settings: two });
    await settle();
    act(() => current.actions.setCallType("technical"));
    await settle();
    expect(current.view.activeProfile?.callType).toBe("technical");
    const patch = s.api.callsOf("setSettings")[0]?.[0] as { profiles: Array<{ id: string; callType: string }> };
    expect(patch.profiles.map((p) => [p.id, p.callType])).toEqual([["default", "technical"], ["p2", "sales"]]);
    act(() => current.actions.setStyle("brief"));
    act(() => current.actions.setLayout("prompter"));
    act(() => current.actions.setActiveProfile("p2"));
    await settle();
    expect(current.view).toMatchObject({ answerStyle: "brief", layout: "prompter" });
    expect(current.view.activeProfile?.id).toBe("p2");
    expect(s.api.core.settings.settingsRevision).toBe(5);
  });

  it("copyDiagnostics copies the text; failures return null with a notice", async () => {
    const s = setup();
    await settle();
    let text: string | null = null;
    await act(async () => {
      text = await current.actions.copyDiagnostics();
    });
    expect(text).toContain("fake core");
    expect(s.copied).toEqual([text]);
    s.api.respond("getDiagnostics", () => ({ ok: false, error: { code: "internal", message: "no diag" } }));
    await act(async () => {
      text = await current.actions.copyDiagnostics();
    });
    expect(text).toBeNull();
    expect(current.view.notice).toBe("no diag");
  });

  it("openExternal failures surface as a notice; dock calls dock_window", async () => {
    const s = setup();
    await settle();
    act(() => current.actions.openExternal("http://insecure"));
    act(() => current.actions.dock());
    await settle();
    expect(current.view.notice).toBe("Only https links can be opened.");
    expect(s.api.callsOf("dockWindow")).toHaveLength(1);
    act(() => current.actions.dismissNotice());
    expect(current.view.notice).toBeNull();
  });
});

describe("refused-stop recovery (§14.3)", () => {
  async function refusedStop(s: Setup) {
    s.api.respond("stopSession", () => ({ ok: false, error: { code: "internal", message: STOP_NOT_TAKEN } }));
    act(() => current.actions.stop());
    await act(async () => {
      for (let i = 0; i < 20; i++) await Promise.resolve();
    });
  }
  function setStatus(s: Setup, id: string | null, phase: StatusSnapshot["session"]["phase"]) {
    s.api.core.status = { ...s.api.core.status, revision: s.api.core.status.revision + 1, session: { id, phase, recording: null } };
  }

  it("core still recording X -> retries the stop -> finalizing", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    expect(current.view.phase).toBe("recording");
    s.api.respond("stopSession", () => ({ ok: true, value: null }));
    await advance(400);
    expect(s.api.callsOf("stopSession")).toEqual([["s1"], ["s1"]]);
    expect(current.view.phase).toBe("finalizing");
    await advance(5_000);
    expect(s.api.callsOf("cancelSession")).toHaveLength(0);
  });

  it("core reports unknown -> waits and asks again; X gone -> idle + cancel_session(X)", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    const polls0 = s.api.callsOf("getStatus").length;
    setStatus(s, null, "unknown");
    await advance(400);
    await advance(400);
    expect(s.api.callsOf("getStatus").length).toBe(polls0 + 2);
    expect(current.view.phase).toBe("recording");
    setStatus(s, null, "idle");
    await advance(400);
    expect(current.view.phase).toBe("idle");
    expect(current.view.notice).toBe(NOTICE.stopNotTaken);
    expect(s.api.callsOf("cancelSession")).toEqual([["s1"]]);
  });

  it("core reports X finalizing/answering -> adopts that phase, no cancel", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    setStatus(s, "s1", "finalizing");
    await advance(400);
    expect(current.view.phase).toBe("finalizing");
    await advance(5_000);
    expect(s.api.callsOf("cancelSession")).toHaveLength(0);
  });

  it("progress for X (a delta) disarms the recovery: no more polls, no cancel", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    const polls0 = s.api.callsOf("getStatus").length;
    emit(s.api, { type: "llm:delta", sessionId: "s1", delta: "Hello" });
    s.runFrame();
    await advance(5_000);
    expect(s.api.callsOf("getStatus").length).toBe(polls0);
    expect(s.api.callsOf("cancelSession")).toHaveLength(0);
    expect(current.view.phase).toBe("answering");
  });

  it("progress for ANOTHER session does not disarm it (scoped to X)", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    const polls0 = s.api.callsOf("getStatus").length;
    emit(s.api, { type: "session:error", sessionId: "s0", error: { code: "aborted", message: "Cancelled." } });
    await advance(400);
    expect(s.api.callsOf("getStatus").length).toBe(polls0 + 1);
  });

  it("an accepted ask disarms the recovery", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    act(() => current.actions.ask("new question"));
    await advance(0);
    const polls0 = s.api.callsOf("getStatus").length;
    await advance(5_000);
    expect(s.api.callsOf("getStatus").length).toBe(polls0);
    expect(s.api.callsOf("cancelSession")).toHaveLength(0);
    expect(current.view.phase).toBe("answering");
  });

  it("bounded: a core that stays unknown gives up after 5 polls -> idle + cancel_session(X)", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    const polls0 = s.api.callsOf("getStatus").length;
    setStatus(s, null, "unknown");
    await advance(400 * 5);
    expect(s.api.callsOf("getStatus").length).toBe(polls0 + 5);
    expect(current.view.phase).toBe("idle");
    expect(s.api.callsOf("cancelSession")).toEqual([["s1"]]);
    await advance(5_000);
    expect(s.api.callsOf("getStatus").length).toBe(polls0 + 5);
  });

  it("a newer session is never touched by an old recovery", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    await refusedStop(s);
    // The core actually moved on and the page got X's terminal error, then a new recording started.
    emit(s.api, { type: "session:error", sessionId: "s1", error: { code: "stt_error", message: "x" } });
    s.api.respond("stopSession", () => ({ ok: true, value: null }));
    await startRecording(s);
    setStatus(s, null, "idle");
    await advance(5_000);
    expect(current.view.phase).toBe("recording");
    expect(s.api.callsOf("cancelSession")).toHaveLength(0);
  });
});

describe("recording clock", () => {
  it("ticks while recording: elapsed timer and silence hint", async () => {
    vi.useFakeTimers();
    const s = setup();
    await advance(0);
    await startRecording(s);
    expect(current.view.statusText).toBe("Recording — 0:00");
    s.clock.t += 5_000;
    await advance(250);
    expect(current.view.statusText).toBe("Recording — 0:05");
    expect(current.view.recording?.silent).toBe(true);
    emit(s.api, { type: "audio:level", sessionId: "s1", rms: 0.3 });
    expect(current.view.recording?.silent).toBe(false);
  });
});

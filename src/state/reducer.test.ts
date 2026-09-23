import { describe, expect, it } from "vitest";
import { makeSettings } from "../app/testing";
import type { CoreEvent } from "../generated/CoreEvent";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import { STATUS, NOTICE } from "./copy";
import { initialState, reducer } from "./reducer";
import { selectView } from "./select";
import type { Action, State } from "./types";

let seq = 0;
function env(event: CoreEvent, s?: number): EventEnvelope {
  seq = s ?? seq + 1;
  return { seq, ...event } as EventEnvelope;
}
function events(state: State, list: EventEnvelope[], now = 1_000): State {
  return reducer(state, { type: "events", envelopes: list, now });
}
function run(state: State, ...actions: Action[]): State {
  return actions.reduce(reducer, state);
}
function snapshot(over: Partial<StatusSnapshot> = {}): StatusSnapshot {
  return {
    revision: 1,
    core: "ready",
    coreError: null,
    protection: "protected",
    session: { id: null, phase: "idle", recording: null },
    ...over,
  };
}
const METRICS = { audioDrainMs: 10, sttFinalizeMs: 20, firstTokenMs: 300, totalMs: 900 };

/** Ready core + settings loaded. */
function ready(): State {
  seq = 0;
  return run(initialState(), { type: "status/snapshot", snapshot: snapshot() }, { type: "settings/adopt", settings: makeSettings() });
}

/** Ready + a started session "s1" in `starting`. */
function started(id = "s1", gen = 1): State {
  return run(ready(), { type: "cmd/start", gen }, { type: "cmd/start-result", gen, result: { ok: true, value: id } });
}

function recording(id = "s1"): State {
  return events(started(id), [env({ type: "session:recording", sessionId: id, deadlineMs: 200_000, capMs: 120_000 })]);
}

function answered(id: string, gen: number, question: string, answer: string, base: State): State {
  let s = run(base, { type: "cmd/ask", gen, question, clickedAtMs: 0 }, { type: "cmd/ask-result", gen, result: { ok: true, value: id } });
  s = events(s, [
    env({ type: "llm:done", sessionId: id, transcript: question, answer, finish: "complete", callType: "behavioral", metrics: METRICS }),
  ]);
  return s;
}

describe("session lifecycle from events", () => {
  it("start ok -> starting until session:recording -> recording with deadline", () => {
    let s = started();
    expect(s.session?.phase).toBe("starting");
    s = events(s, [env({ type: "session:recording", sessionId: "s1", deadlineMs: 200_000, capMs: 120_000 })]);
    expect(s.session?.phase).toBe("recording");
    expect(s.session?.deadlineMs).toBe(200_000);
    expect(s.session?.startedAtMs).toBe(80_000);
  });

  it("stt:partial replaces the transcript with the full text", () => {
    let s = recording();
    s = events(s, [env({ type: "stt:partial", sessionId: "s1", text: "hello", isFinal: false })]);
    s = events(s, [env({ type: "stt:partial", sessionId: "s1", text: "hello there", isFinal: true })]);
    expect(s.session?.transcript).toBe("hello there");
  });

  it("audio:level tracks rms and the last loud time", () => {
    let s = recording();
    s = events(s, [env({ type: "audio:level", sessionId: "s1", rms: 0.2 })], 5_000);
    expect(s.session?.level).toBe(0.2);
    expect(s.session?.lastLoudAtMs).toBe(5_000);
    s = events(s, [env({ type: "audio:level", sessionId: "s1", rms: 0.001 })], 6_000);
    expect(s.session?.lastLoudAtMs).toBe(5_000);
  });

  it("stop ok -> finalizing; first delta -> answering; llm:done -> idle + history", () => {
    let s = recording();
    s = run(s, { type: "cmd/stop", gen: 2, clickedAtMs: 100 });
    expect(s.session?.stopPending).toBe(true);
    s = run(s, { type: "cmd/stop-result", sessionId: "s1", result: { ok: true, value: null } });
    expect(s.session?.phase).toBe("finalizing");
    s = events(s, [env({ type: "stt:partial", sessionId: "s1", text: "Why us?", isFinal: true })]);
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "Because " })]);
    expect(s.session?.phase).toBe("answering");
    expect(s.session?.streaming).toBe(true);
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "I like it." })]);
    expect(s.session?.answer).toBe("Because I like it.");
    s = events(s, [
      env({ type: "llm:done", sessionId: "s1", transcript: "Why us?", answer: "Because I like it.", finish: "truncated", callType: "technical", metrics: METRICS }),
    ]);
    expect(s.session?.phase).toBe("idle");
    expect(s.history).toHaveLength(1);
    expect(s.history[0]).toMatchObject({ question: "Why us?", answer: "Because I like it.", finish: "truncated", callType: "technical", metrics: METRICS });
  });

  it("session:error -> idle; pushed to history only when there is a question or answer", () => {
    let s = recording();
    s = events(s, [env({ type: "session:error", sessionId: "s1", error: { code: "no_speech", message: "No speech" } })]);
    expect(s.session?.phase).toBe("idle");
    expect(s.session?.error?.code).toBe("no_speech");
    expect(s.history).toHaveLength(0);

    let t = recording("s1");
    t = events(t, [env({ type: "stt:partial", sessionId: "s1", text: "q?", isFinal: true })]);
    t = events(t, [env({ type: "session:error", sessionId: "s1", error: { code: "llm_http", message: "boom" } })]);
    expect(t.history).toHaveLength(1);
    expect(t.history[0]?.error?.code).toBe("llm_http");
  });

  it("at most one terminal per session: a second terminal is ignored", () => {
    let s = recording();
    s = events(s, [env({ type: "stt:partial", sessionId: "s1", text: "q", isFinal: true })]);
    s = events(s, [env({ type: "llm:done", sessionId: "s1", transcript: "q", answer: "a", finish: "complete", callType: "behavioral", metrics: METRICS })]);
    s = events(s, [env({ type: "session:error", sessionId: "s1", error: { code: "stt_error", message: "late" } })]);
    expect(s.session?.error).toBeNull();
    expect(s.history).toHaveLength(1);
  });

  it("session:autostopped -> finalizing + cap-reached status", () => {
    let s = recording();
    s = events(s, [env({ type: "session:autostopped", sessionId: "s1" })]);
    expect(s.session?.phase).toBe("finalizing");
    expect(selectView(s, 0).statusText).toBe(STATUS.capReached);
  });

  it("audio:device lost -> notice + finalizing (not the 120s copy); changed -> notice only", () => {
    let s = recording();
    s = events(s, [env({ type: "audio:device", sessionId: "s1", kind: "changed", message: "Device changed" })]);
    expect(s.notice).toBe("Device changed");
    expect(s.session?.phase).toBe("recording");
    s = events(s, [env({ type: "audio:device", sessionId: "s1", kind: "lost", message: "Device lost" })]);
    s = events(s, [env({ type: "session:autostopped", sessionId: "s1" })]);
    expect(s.notice).toBe("Device lost");
    expect(s.session?.phase).toBe("finalizing");
    expect(selectView(s, 0).statusText).toBe(STATUS.finalizing);
  });

  it("ask: starts directly in answering with the question as transcript", () => {
    let s = run(ready(), { type: "cmd/ask", gen: 1, question: "What is Rust?", clickedAtMs: 5 });
    expect(s.session?.phase).toBe("answering");
    expect(s.session?.transcript).toBe("What is Rust?");
    s = run(s, { type: "cmd/ask-result", gen: 1, result: { ok: true, value: "s4" } });
    expect(s.session?.id).toBe("s4");
  });
});

describe("seq, stale sessions and buffering", () => {
  it("drops envelopes with seq <= lastSeq (duplicates / replays)", () => {
    let s = recording();
    const e = env({ type: "llm:delta", sessionId: "s1", delta: "x" });
    s = events(s, [e, e]);
    s = events(s, [e]);
    expect(s.session?.answer).toBe("x");
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "OLD" }, 1)]);
    expect(s.session?.answer).toBe("x");
  });

  it("lastSeq starts at 0 on load and is never reset by snapshots or core:ready", () => {
    expect(initialState().lastSeq).toBe(0);
    let s = recording();
    s = events(s, [env({ type: "core:ready", revision: 5 }, 50)]);
    s = run(s, { type: "status/snapshot", snapshot: snapshot({ revision: 9 }) }, { type: "settings/adopt", settings: makeSettings({ settingsRevision: 3 }) });
    expect(s.lastSeq).toBe(50);
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "dup" }, 49)]);
    expect(s.session?.answer).toBe("");
  });

  it("events for a session id that is not current are dropped (§5.3)", () => {
    let s = recording("s2");
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "stale" })]);
    s = events(s, [env({ type: "session:error", sessionId: "s1", error: { code: "aborted", message: "Cancelled." } })]);
    expect(s.session?.answer).toBe("");
    expect(s.session?.phase).toBe("recording");
  });

  it("events arriving before the start result are buffered and replayed only if they are ours", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = events(s, [
      env({ type: "session:error", sessionId: "s1", error: { code: "aborted", message: "Cancelled." } }),
      env({ type: "session:recording", sessionId: "s2", deadlineMs: 150_000, capMs: 120_000 }),
    ]);
    expect(s.session?.phase).toBe("starting");
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s2" } });
    expect(s.session?.phase).toBe("recording");
    expect(s.session?.error).toBeNull();
    expect(s.pending).toHaveLength(0);
  });
});

describe("events that precede the command reply (orchestrator note)", () => {
  it("ask: the question's final stt:partial emitted during install is replayed once the reply adopts the id", () => {
    let s = run(ready(), { type: "cmd/ask", gen: 1, question: "  typed  ", clickedAtMs: 0 });
    s = events(s, [
      env({ type: "stt:partial", sessionId: "s3", text: "Typed question?", isFinal: true }),
      env({ type: "llm:delta", sessionId: "s3", delta: "Early " }),
    ]);
    expect(s.pending).toHaveLength(2);
    s = run(s, { type: "cmd/ask-result", gen: 1, result: { ok: true, value: "s3" } });
    expect(s.session).toMatchObject({ id: "s3", transcript: "Typed question?", answer: "Early ", phase: "answering" });
    expect(s.pending).toHaveLength(0);
  });

  it("record: session:recording + early partial/level before the start reply are replayed in order", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = events(
      s,
      [
        env({ type: "session:recording", sessionId: "s2", deadlineMs: 150_000, capMs: 120_000 }),
        env({ type: "audio:level", sessionId: "s2", rms: 0.5 }),
        env({ type: "stt:partial", sessionId: "s2", text: "hel", isFinal: false }),
        env({ type: "stt:partial", sessionId: "s2", text: "hello", isFinal: false }),
      ],
      42_000,
    );
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s2" } });
    expect(s.session).toMatchObject({ phase: "recording", transcript: "hello", level: 0.5, lastLoudAtMs: 42_000, deadlineMs: 150_000 });
  });

  it("buffered events are dropped when the command fails", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = events(s, [env({ type: "session:recording", sessionId: "s2", deadlineMs: 150_000, capMs: 120_000 })]);
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: false, error: { code: "no_stt_key", message: "k" } } });
    expect(s.pending).toHaveLength(0);
    expect(s.session?.phase).toBe("idle");
    expect(s.session?.deadlineMs).toBeNull();
  });

  it("buffered events are dropped when the command is superseded; the newer command gets only its own", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = events(s, [env({ type: "session:recording", sessionId: "s2", deadlineMs: 150_000, capMs: 120_000 })]);
    s = run(s, { type: "cmd/ask", gen: 2, question: "q", clickedAtMs: 0 });
    expect(s.pending).toHaveLength(0);
    s = events(s, [
      env({ type: "stt:partial", sessionId: "s2", text: "old", isFinal: false }),
      env({ type: "stt:partial", sessionId: "s3", text: "q", isFinal: true }),
    ]);
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s2" } });
    s = run(s, { type: "cmd/ask-result", gen: 2, result: { ok: true, value: "s3" } });
    expect(s.session).toMatchObject({ id: "s3", transcript: "q", phase: "answering", deadlineMs: null });
  });

  it("a superseded session needs no terminal event: the UI has already moved on locally", () => {
    let s = recording("s1");
    s = run(s, { type: "cmd/ask", gen: 5, question: "next", clickedAtMs: 0 });
    expect(s.session?.phase).toBe("answering");
    expect(s.endedIds).toContain("s1");
    const c = run(recording("s1"), { type: "cmd/cancel", gen: 9 });
    expect(selectView(c, 0).phase).toBe("idle");
  });
});

describe("command generations (§14.2)", () => {
  it("a late start-failed must not flip a newer session to idle", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = run(s, { type: "cmd/ask", gen: 2, question: "q", clickedAtMs: 0 });
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: false, error: { code: "internal", message: "late failure" } } });
    expect(s.session?.gen).toBe(2);
    expect(s.session?.phase).toBe("answering");
    expect(s.session?.error).toBeNull();
  });

  it("a late start-aborted / start-ok must not null or hijack a newer session", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = run(s, { type: "cmd/ask", gen: 2, question: "q", clickedAtMs: 0 });
    s = run(s, { type: "cmd/ask-result", gen: 2, result: { ok: true, value: "s2" } });
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: false, error: { code: "aborted", message: "Cancelled." } } });
    expect(s.session?.id).toBe("s2");
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s1" } });
    expect(s.session?.id).toBe("s2");
  });

  it("the current command's failure shows the error and returns to idle", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 });
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: false, error: { code: "no_stt_key", message: "Add key" } } });
    expect(s.session?.phase).toBe("idle");
    expect(selectView(s, 0).sessionError?.code).toBe("no_stt_key");
  });

  it("a stop is a command generation but does not orphan the pending start's slot", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 }, { type: "cmd/stop", gen: 2, clickedAtMs: 1 });
    expect(s.cmdGen).toBe(2);
    expect(s.session?.stopQueued).toBe(true);
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s1" } });
    expect(s.session?.id).toBe("s1");
    expect(s.session?.stopQueued).toBe(true);
  });

  it("a stop result for another session is ignored", () => {
    let s = recording("s2");
    s = run(s, { type: "cmd/stop-result", sessionId: "s1", result: { ok: true, value: null } });
    expect(s.session?.phase).toBe("recording");
  });

  it("cancel while start is pending leaves an idle slot that ignores the late result", () => {
    let s = run(ready(), { type: "cmd/start", gen: 1 }, { type: "cmd/cancel", gen: 2 });
    expect(s.session?.phase).toBe("idle");
    s = run(s, { type: "cmd/start-result", gen: 1, result: { ok: true, value: "s1" } });
    expect(s.session?.id).toBeNull();
  });
});

describe("revision adoption", () => {
  it("adopts snapshots/events only when revision >= last seen", () => {
    let s = run(initialState(), { type: "status/snapshot", snapshot: snapshot({ revision: 5, protection: "protected" }) });
    s = run(s, { type: "status/snapshot", snapshot: snapshot({ revision: 4, protection: "unprotected", core: "failed" }) });
    expect(s.protection).toBe("protected");
    expect(s.core).toBe("ready");
    s = events(s, [env({ type: "protection:failed", revision: 3, protection: "unprotected" })]);
    expect(s.protection).toBe("protected");
    s = events(s, [env({ type: "protection:failed", revision: 5, protection: "unprotected" })]);
    expect(s.protection).toBe("unprotected");
    s = events(s, [env({ type: "core:failed", revision: 6, error: { code: "internal", message: "Core broke" } })]);
    expect(s.core).toBe("failed");
    expect(selectView(s, 0).statusText).toBe("Core broke");
    s = events(s, [env({ type: "core:ready", revision: 2 })]);
    expect(s.core).toBe("failed");
  });

  it("re-adopts a live session from the snapshot, including its deadline", () => {
    const s = run(initialState(), {
      type: "status/snapshot",
      snapshot: snapshot({ revision: 3, session: { id: "s7", phase: "recording", recording: { deadlineMs: 500_000, capMs: 120_000 } } }),
    });
    expect(s.session).toMatchObject({ id: "s7", phase: "recording", deadlineMs: 500_000, capMs: 120_000, startedAtMs: 380_000 });
    const v = selectView(s, 390_000);
    expect(v.recording?.deadlineMs).toBe(500_000);
    expect(v.statusText).toBe("Recording — 0:10");
  });

  it("does not re-adopt an ended session or replace a live one from a snapshot", () => {
    let s = answered("s1", 1, "q", "a", ready());
    s = run(s, { type: "status/snapshot", snapshot: snapshot({ revision: 2, session: { id: "s1", phase: "answering", recording: null } }) });
    expect(s.session?.phase).toBe("idle");
    let t = recording("s2");
    t = run(t, { type: "status/snapshot", snapshot: snapshot({ revision: 2, session: { id: "s9", phase: "recording", recording: null } }) });
    expect(t.session?.id).toBe("s2");
  });

  it("settings adopt only when settingsRevision >= current", () => {
    let s = run(initialState(), { type: "settings/adopt", settings: makeSettings({ settingsRevision: 4, answerStyle: "brief" }) });
    s = run(s, { type: "settings/adopt", settings: makeSettings({ settingsRevision: 3, answerStyle: "detailed" }) });
    expect(s.settings?.answerStyle).toBe("brief");
  });
});

describe("history", () => {
  it("caps at 6 entries, dropping the oldest", () => {
    let s = ready();
    for (let i = 1; i <= 8; i++) s = answered(`s${i}`, i, `q${i}`, `a${i}`, s);
    expect(s.history).toHaveLength(6);
    expect(s.history[0]?.question).toBe("q3");
    expect(selectView(s, 0).history).toEqual({ index: 6, count: 6, canPrev: true, canNext: false });
  });

  it("prev/next navigation walks entries and returns to live", () => {
    let s = ready();
    for (let i = 1; i <= 3; i++) s = answered(`s${i}`, i, `q${i}`, `a${i}`, s);
    s = run(s, { type: "history/prev" });
    expect(selectView(s, 0).entry?.question).toBe("q2");
    expect(selectView(s, 0).history).toEqual({ index: 2, count: 3, canPrev: true, canNext: true });
    s = run(s, { type: "history/prev" }, { type: "history/prev" });
    expect(s.historyIndex).toBe(0);
    expect(selectView(s, 0).history.canPrev).toBe(false);
    s = run(s, { type: "history/next" }, { type: "history/next" });
    expect(s.historyIndex).toBeNull();
    expect(selectView(s, 0).entry?.question).toBe("q3");
  });

  it("clear empties history and the finished live entry", () => {
    let s = answered("s1", 1, "q", "a", ready());
    s = run(s, { type: "history/clear" });
    expect(s.history).toHaveLength(0);
    expect(selectView(s, 0).entry).toBeNull();
    expect(selectView(s, 0).history.count).toBe(0);
  });

  it("a new session jumps the view back to live", () => {
    let s = ready();
    for (let i = 1; i <= 2; i++) s = answered(`s${i}`, i, `q${i}`, `a${i}`, s);
    s = run(s, { type: "history/prev" }, { type: "cmd/start", gen: 3 });
    expect(s.historyIndex).toBeNull();
  });

  it("regenerate (ask with the viewed question) creates a NEW entry, never overwrites", () => {
    let s = ready();
    s = answered("s1", 1, "Tell me about you", "first", s);
    s = answered("s2", 2, "Other", "second", s);
    s = run(s, { type: "history/prev" });
    const v = selectView(s, 0);
    expect(v.canRegenerate).toBe(true);
    expect(v.entry?.question).toBe("Tell me about you");
    s = answered("s3", 3, v.entry?.question ?? "", "third", s);
    expect(s.history.map((h) => h.answer)).toEqual(["first", "second", "third"]);
    expect(s.history[2]?.question).toBe("Tell me about you");
  });

  it("history is view state only: entries hold no profile/settings data", () => {
    const s = answered("s1", 1, "q", "a", ready());
    expect(Object.keys(s.history[0] ?? {}).sort()).toEqual(
      ["answer", "callType", "error", "finish", "metrics", "question", "sessionId", "visibleFirstWordMs"].sort(),
    );
  });
});

describe("first paint + misc", () => {
  it("visibleFirstWordMs is measured once from the stop click", () => {
    let s = recording();
    s = run(s, { type: "cmd/stop", gen: 2, clickedAtMs: 1_000 }, { type: "cmd/stop-result", sessionId: "s1", result: { ok: true, value: null } });
    s = run(s, { type: "first-paint", atMs: 1_100 });
    expect(s.session?.visibleFirstWordMs).toBeNull(); // nothing painted yet
    s = events(s, [env({ type: "llm:delta", sessionId: "s1", delta: "Hi" })]);
    s = run(s, { type: "first-paint", atMs: 1_750 }, { type: "first-paint", atMs: 2_000 });
    expect(s.session?.visibleFirstWordMs).toBe(750);
  });

  it("window:close-requested sets closeRequested; dismiss clears it", () => {
    let s = events(ready(), [env({ type: "window:close-requested" })]);
    expect(s.closeRequested).toBe(true);
    s = run(s, { type: "close/dismiss" });
    expect(s.closeRequested).toBe(false);
  });

  it("closing settings clears the dirty flag", () => {
    const s = run(ready(), { type: "screen", screen: "settings" }, { type: "settings/dirty", dirty: true }, { type: "screen", screen: "main" });
    expect(s.settingsDirty).toBe(false);
  });

  it("recovery/gone returns the scoped session to idle with a notice; other ids untouched", () => {
    let s = recording("s2");
    s = run(s, { type: "recovery/gone", sessionId: "s1", notice: NOTICE.stopNotTaken });
    expect(s.session?.phase).toBe("recording");
    s = run(s, { type: "recovery/gone", sessionId: "s2", notice: NOTICE.stopNotTaken });
    expect(s.session?.phase).toBe("idle");
    expect(s.notice).toBe(NOTICE.stopNotTaken);
  });
});

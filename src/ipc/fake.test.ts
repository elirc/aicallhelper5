import { afterEach, describe, expect, it, vi } from "vitest";
import type { EventEnvelope } from "../generated/EventEnvelope";
import { createFakeApi, STALE_REVISION, STOP_NOT_TAKEN } from "./fake";

afterEach(() => vi.useRealTimers());

describe("createFakeApi", () => {
  it("emit assigns increasing seq and reaches only active listeners", () => {
    const api = createFakeApi();
    const got: EventEnvelope[] = [];
    const unsub = api.subscribe((e) => got.push(e));
    api.emit({ type: "core:ready", revision: 1 });
    api.emit({ type: "settings:changed" });
    unsub();
    api.emit({ type: "settings:changed" });
    expect(got.map((e) => e.seq)).toEqual([1, 2]);
    expect(api.listenerCount()).toBe(0);
  });

  it("logs calls and lets tests override responses", async () => {
    const api = createFakeApi();
    api.respond("startSession", () => ({ ok: false, error: { code: "no_stt_key", message: "k" } }));
    expect((await api.startSession()).ok).toBe(false);
    await api.stopSession("s1");
    expect(api.callsOf("stopSession")).toEqual([["s1"]]);
    expect(api.calls.map((c) => c.method)).toEqual(["startSession", "stopSession"]);
  });

  it("set_settings enforces baseRevision and bumps settingsRevision", async () => {
    const api = createFakeApi();
    const r = await api.setSettings({ baseRevision: 1, answerStyle: "brief" });
    expect(r.ok && r.value.settingsRevision).toBe(2);
    expect(await api.setSettings({ baseRevision: 1, answerStyle: "detailed" })).toEqual({
      ok: false,
      error: { code: "internal", message: STALE_REVISION },
    });
  });

  it("stop is only taken for the live session", async () => {
    const api = createFakeApi();
    const s = await api.startSession();
    expect(s).toEqual({ ok: true, value: "s1" });
    expect((await api.stopSession("s9")).ok).toBe(false);
    expect(await api.stopSession("s1")).toEqual({ ok: true, value: null });
    expect(await api.stopSession("s1")).toEqual({ ok: false, error: { code: "internal", message: STOP_NOT_TAKEN } });
  });

  it("demo mode plays start -> recording -> partials -> stop -> deltas -> done", async () => {
    vi.useFakeTimers();
    const api = createFakeApi({ demo: true });
    const types: string[] = [];
    api.subscribe((e) => types.push(e.type));
    await api.startSession();
    await vi.advanceTimersByTimeAsync(4_000);
    await api.stopSession("s1");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(types).toContain("session:recording");
    expect(types).toContain("stt:partial");
    expect(types.filter((t) => t === "llm:delta").length).toBeGreaterThan(5);
    expect(types[types.length - 1]).toBe("llm:done");
    expect(api.core.status.session.phase).toBe("idle");
    api.dispose();
  });
});

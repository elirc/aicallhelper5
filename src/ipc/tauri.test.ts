import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const channels: FakeChannel[] = [];
class FakeChannel {
  onmessage: (m: unknown) => void = () => {};
  constructor() {
    channels.push(this);
  }
}
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: FakeChannel,
}));

const { createTauriApi, normalizeResult, COMMAND_TIMEOUT_MS, TIMEOUT_MESSAGE } = await import("./tauri");
const { makeSettings } = await import("../app/testing");

beforeEach(() => {
  invoke.mockReset();
  channels.length = 0;
});
afterEach(() => {
  vi.useRealTimers();
});

describe("createTauriApi", () => {
  it("passes the exact command names and argument objects (incl. sessionId)", async () => {
    invoke.mockResolvedValue({ ok: true, value: null });
    const api = createTauriApi();
    const patch = { baseRevision: 3, answerStyle: "brief" as const };
    await api.setSettings(patch);
    await api.stopSession("s1");
    await api.cancelSession("s2");
    await api.setCloseGuard(true);
    await api.openExternal("https://example.com");
    await api.dockWindow();
    invoke.mockResolvedValue({ ok: true, value: "s9" });
    await api.startSession();
    await api.ask("hi");
    expect(invoke.mock.calls).toEqual([
      ["set_settings", { patch }],
      ["stop_session", { sessionId: "s1" }],
      ["cancel_session", { sessionId: "s2" }],
      ["set_close_guard", { active: true }],
      ["open_external", { url: "https://example.com" }],
      ["dock_window", undefined],
      ["start_session", undefined],
      ["ask", { text: "hi" }],
    ]);
  });

  it("returns the CmdResult the Rust side sends", async () => {
    const api = createTauriApi();
    invoke.mockResolvedValueOnce({ ok: true, value: "s3" });
    expect(await api.startSession()).toEqual({ ok: true, value: "s3" });
    invoke.mockResolvedValueOnce({ ok: false, error: { code: "no_stt_key", message: "Add key" } });
    expect(await api.startSession()).toEqual({ ok: false, error: { code: "no_stt_key", message: "Add key" } });
    invoke.mockResolvedValueOnce({ ok: true, value: makeSettings() });
    expect((await api.getSettings()).ok).toBe(true);
  });

  it("never rejects: thrown / rejected invokes map to internal", async () => {
    const api = createTauriApi();
    invoke.mockRejectedValueOnce("command stop_session not found");
    expect(await api.stopSession("s1")).toEqual({ ok: false, error: { code: "internal", message: "command stop_session not found" } });
    invoke.mockImplementationOnce(() => {
      throw new Error("no __TAURI_INTERNALS__");
    });
    expect(await api.getStatus()).toEqual({ ok: false, error: { code: "internal", message: "no __TAURI_INTERNALS__" } });
    invoke.mockRejectedValueOnce({ weird: true });
    const r = await api.getDiagnostics();
    expect(r.ok).toBe(false);
  });

  it("validates the response shape defensively", async () => {
    const api = createTauriApi();
    for (const bad of [undefined, null, 42, "s1", { ok: "yes" }, { ok: true, value: 5 }, { ok: false, error: "x" }]) {
      invoke.mockResolvedValueOnce(bad);
      const r = await api.startSession();
      expect(r.ok).toBe(false);
      if (!r.ok) expect(r.error.code).toBe("internal");
    }
    invoke.mockResolvedValueOnce({ ok: true, value: { revision: 1 } });
    expect((await api.getStatus()).ok).toBe(false);
    invoke.mockResolvedValueOnce({ ok: false, error: { code: "made_up", message: "m" } });
    expect(await api.ask("x")).toEqual({ ok: false, error: { code: "internal", message: "m" } });
  });

  it("resolves to internal after the 30 s client-side timeout", async () => {
    vi.useFakeTimers();
    invoke.mockReturnValue(new Promise(() => {}));
    const api = createTauriApi();
    const p = api.getStatus();
    let settled = false;
    void p.then(() => (settled = true));
    await vi.advanceTimersByTimeAsync(COMMAND_TIMEOUT_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await p).toEqual({ ok: false, error: { code: "internal", message: TIMEOUT_MESSAGE } });
  });

  it("subscribe attaches a Channel via subscribe_events and unsubscribe stops forwarding", async () => {
    invoke.mockResolvedValue({ ok: true, value: null });
    const api = createTauriApi();
    const got: unknown[] = [];
    const unsub = api.subscribe((e) => got.push(e));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
    const [cmd, args] = invoke.mock.calls[0] as [string, { channel: unknown }];
    expect(cmd).toBe("subscribe_events");
    expect(args.channel).toBe(channels[0]);
    channels[0]?.onmessage({ seq: 1, type: "core:ready", revision: 1 });
    channels[0]?.onmessage({ nonsense: true });
    unsub();
    channels[0]?.onmessage({ seq: 2, type: "core:ready", revision: 2 });
    expect(got).toEqual([{ seq: 1, type: "core:ready", revision: 1 }]);
  });

  it("retries a failed subscribe_events attach (bounded)", async () => {
    vi.useFakeTimers();
    invoke.mockResolvedValue({ ok: false, error: { code: "internal", message: "not ready" } });
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    createTauriApi().subscribe(() => {});
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invoke).toHaveBeenCalledTimes(3);
    err.mockRestore();
  });
});

describe("normalizeResult", () => {
  it("treats a missing value on ok as null (unit commands)", () => {
    expect(normalizeResult({ ok: true }, (v): v is null => v === null)).toEqual({ ok: true, value: null });
  });
});

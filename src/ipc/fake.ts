// Scriptable in-memory core. Used by tests (drive events by hand with
// `emit`, override commands with `respond`, inspect `calls`) and by
// `npm run dev` in a plain browser (`demo: true` plays a scripted session:
// start -> partials -> stop -> streamed deltas -> done).
import { makeSettings } from "../app/testing";
import type { CoreEvent } from "../generated/CoreEvent";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { SessionId } from "../generated/SessionId";
import type { SettingsPatch } from "../generated/SettingsPatch";
import type { SettingsView } from "../generated/SettingsView";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import type { CmdResult, CoreApi } from "./types";

export type FakeMethod = Exclude<keyof CoreApi, "subscribe">;
export interface FakeCall {
  method: FakeMethod | "subscribe";
  args: unknown[];
}

type Handler<M extends FakeMethod> = (
  ...args: Parameters<CoreApi[M]>
) => Awaited<ReturnType<CoreApi[M]>> | ReturnType<CoreApi[M]>;

export interface FakeApiOptions {
  settings?: SettingsView;
  status?: Partial<StatusSnapshot>;
  /** Play a scripted demo session (browser dev mode). */
  demo?: boolean;
  /** Demo pacing multiplier (1 = realistic). */
  demoSpeed?: number;
}

export interface FakeApi extends CoreApi {
  /** Deliver an event to every active listener with the next `seq`. */
  emit(event: CoreEvent): EventEnvelope;
  /** Deliver a raw envelope (e.g. a duplicate / out-of-order seq). */
  emitEnvelope(env: EventEnvelope): void;
  /** Override a command's behavior. */
  respond<M extends FakeMethod>(method: M, handler: Handler<M>): void;
  /** Every call, in order. */
  readonly calls: FakeCall[];
  callsOf(method: FakeMethod | "subscribe"): unknown[][];
  listenerCount(): number;
  /** Current fake core state (mutable for tests). */
  readonly core: { settings: SettingsView; status: StatusSnapshot; seq: number; nextSession: number };
  /** Stop all demo timers. */
  dispose(): void;
}

export const STOP_NOT_TAKEN = "That recording is no longer active.";
export const STALE_REVISION = "Settings changed since they were loaded, so nothing was saved (stale revision).";

const DEMO_QUESTION = [
  "So tell me about",
  "So tell me about a time you",
  "So tell me about a time you had to deal with",
  "So tell me about a time you had to deal with a difficult deadline.",
];
const DEMO_ANSWER =
  "Sure — at my last job we had **two weeks** to ship a billing migration that was scoped for six.\n\n" +
  "- I split the work into a *must-ship* core and nice-to-haves\n" +
  "- I set up a daily 10-minute check-in with finance\n" +
  "- I moved the risky data backfill to run behind a flag\n\n" +
  "We shipped the core on time, with `zero` billing errors, and the rest landed the following sprint.";

function ok<T>(value: T): CmdResult<T> {
  return { ok: true, value };
}
function fail<T>(message: string): CmdResult<T> {
  return { ok: false, error: { code: "internal", message } };
}

export function createFakeApi(opts: FakeApiOptions = {}): FakeApi {
  const listeners = new Set<(ev: EventEnvelope) => void>();
  const handlers = new Map<FakeMethod, (...args: never[]) => unknown>();
  const calls: FakeCall[] = [];
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const speed = opts.demoSpeed ?? 1;

  const core = {
    settings: opts.settings ?? makeSettings(),
    status: {
      revision: 1,
      core: "ready",
      coreError: null,
      protection: "protected",
      session: { id: null, phase: "idle", recording: null },
      ...opts.status,
    } as StatusSnapshot,
    seq: 0,
    nextSession: 1,
  };

  function later(ms: number, fn: () => void) {
    const t = setTimeout(() => {
      timers.delete(t);
      fn();
    }, ms * speed);
    timers.add(t);
  }

  function emitEnvelope(env: EventEnvelope) {
    for (const l of [...listeners]) l(env);
  }

  function emit(event: CoreEvent): EventEnvelope {
    core.seq += 1;
    const env = { seq: core.seq, ...event } as EventEnvelope;
    emitEnvelope(env);
    return env;
  }

  function setSession(id: SessionId | null, phase: StatusSnapshot["session"]["phase"], recording: StatusSnapshot["session"]["recording"] = null) {
    core.status = { ...core.status, revision: core.status.revision + 1, session: { id, phase, recording } };
  }

  function isCurrent(id: SessionId) {
    return core.status.session.id === id && core.status.session.phase !== "idle";
  }

  function streamAnswer(id: SessionId, question: string, startDelay: number) {
    const words = DEMO_ANSWER.match(/\S+\s*/g) ?? [];
    let t = startDelay;
    let first = true;
    for (const w of words) {
      t += 40;
      later(t, () => {
        if (!isCurrent(id)) return;
        if (first) {
          first = false;
          setSession(id, "answering");
        }
        emit({ type: "llm:delta", sessionId: id, delta: w });
      });
    }
    later(t + 60, () => {
      if (!isCurrent(id)) return;
      setSession(null, "idle");
      emit({
        type: "llm:done",
        sessionId: id,
        transcript: question,
        answer: DEMO_ANSWER,
        finish: "complete",
        callType: core.settings.profiles.find((p) => p.id === core.settings.activeProfileId)?.callType ?? "behavioral",
        metrics: { audioDrainMs: 42, sttFinalizeMs: 180, firstTokenMs: startDelay + 40, totalMs: t + 60 },
      });
    });
  }

  const defaults: { [M in FakeMethod]: Handler<M> } = {
    getSettings: () => ok(core.settings),
    setSettings: (patch: SettingsPatch) => {
      if (patch.baseRevision !== core.settings.settingsRevision) return fail(STALE_REVISION);
      const { baseRevision: _base, secrets, ...rest } = patch;
      void _base;
      let keys = core.settings.keys;
      for (const s of secrets ?? []) {
        keys = keys.map((k) =>
          k.id === s.keyId
            ? { ...k, hasKey: s.action === "set", storage: s.action === "set" ? ("encrypted" as const) : ("unset" as const) }
            : k,
        );
      }
      const defined = Object.fromEntries(Object.entries(rest).filter(([, v]) => v !== undefined));
      core.settings = { ...core.settings, ...defined, keys, settingsRevision: core.settings.settingsRevision + 1 };
      return ok(core.settings);
    },
    getStatus: () => ok(core.status),
    startSession: () => {
      // Like the real core: a superseded session emits nothing further.
      const id = `s${core.nextSession++}`;
      setSession(id, "starting");
      if (opts.demo) {
        later(300, () => {
          if (!isCurrent(id)) return;
          const capMs = 120_000;
          const deadlineMs = Date.now() + capMs;
          setSession(id, "recording", { deadlineMs, capMs });
          emit({ type: "session:recording", sessionId: id, deadlineMs, capMs });
        });
        DEMO_QUESTION.forEach((text, i) => {
          later(900 + i * 700, () => {
            if (isCurrent(id) && core.status.session.phase === "recording") {
              emit({ type: "stt:partial", sessionId: id, text, isFinal: i === DEMO_QUESTION.length - 1 });
            }
          });
        });
        for (let i = 0; i < 40; i++) {
          later(400 + i * 128, () => {
            if (isCurrent(id) && core.status.session.phase === "recording") {
              emit({ type: "audio:level", sessionId: id, rms: 0.05 + 0.2 * Math.abs(Math.sin(i / 3)) });
            }
          });
        }
      }
      return ok(id);
    },
    stopSession: (sessionId: SessionId) => {
      const s = core.status.session;
      if (s.id !== sessionId || (s.phase !== "starting" && s.phase !== "recording")) return fail(STOP_NOT_TAKEN);
      setSession(sessionId, "finalizing");
      if (opts.demo) streamAnswer(sessionId, DEMO_QUESTION[DEMO_QUESTION.length - 1] ?? "", 500);
      return ok(null);
    },
    ask: (text: string) => {
      const q = text.trim();
      if (!q) return fail("Type a question first.");
      const id = `s${core.nextSession++}`;
      setSession(id, "answering");
      if (opts.demo) {
        later(10, () => {
          if (isCurrent(id)) emit({ type: "stt:partial", sessionId: id, text: q, isFinal: true });
        });
        streamAnswer(id, q, 400);
      }
      return ok(id);
    },
    cancelSession: (sessionId: SessionId) => {
      if (isCurrent(sessionId)) setSession(null, "idle");
      return ok(null);
    },
    setCloseGuard: () => ok(null),
    dockWindow: () => ok(null),
    openExternal: (url: string) => (/^https:\/\/[^\s/]+/.test(url) ? ok(null) : fail("Only https links can be opened.")),
    getDiagnostics: () => ok(`AI Call Assistant ${core.settings.build.version} (fake core)\nprotection: ${core.status.protection}`),
  };

  async function run<M extends FakeMethod>(method: M, ...args: Parameters<CoreApi[M]>): Promise<Awaited<ReturnType<CoreApi[M]>>> {
    calls.push({ method, args });
    const h = (handlers.get(method) ?? defaults[method]) as unknown as (...a: Parameters<CoreApi[M]>) => unknown;
    try {
      return (await h(...args)) as Awaited<ReturnType<CoreApi[M]>>;
    } catch (e) {
      return fail(e instanceof Error ? e.message : String(e)) as Awaited<ReturnType<CoreApi[M]>>;
    }
  }

  return {
    getSettings: () => run("getSettings"),
    setSettings: (patch) => run("setSettings", patch),
    getStatus: () => run("getStatus"),
    startSession: () => run("startSession"),
    stopSession: (id) => run("stopSession", id),
    ask: (text) => run("ask", text),
    cancelSession: (id) => run("cancelSession", id),
    setCloseGuard: (active) => run("setCloseGuard", active),
    dockWindow: () => run("dockWindow"),
    openExternal: (url) => run("openExternal", url),
    getDiagnostics: () => run("getDiagnostics"),
    subscribe(listener) {
      calls.push({ method: "subscribe", args: [] });
      let active = true;
      const wrapped = (ev: EventEnvelope) => {
        if (active) listener(ev);
      };
      listeners.add(wrapped);
      return () => {
        active = false;
        listeners.delete(wrapped);
      };
    },
    emit,
    emitEnvelope,
    respond(method, handler) {
      handlers.set(method, handler as unknown as (...args: never[]) => unknown);
    },
    calls,
    callsOf: (method) => calls.filter((c) => c.method === method).map((c) => c.args),
    listenerCount: () => listeners.size,
    core,
    dispose() {
      for (const t of timers) clearTimeout(t);
      timers.clear();
    },
  };
}

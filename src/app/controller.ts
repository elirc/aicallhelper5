// Side-effect half of the store: commands, event intake (seq dedupe + delta
// coalescing), refused-stop recovery, serialized settings saves. Framework-
// free so it can be unit-tested; AppProvider wires it to React.
import type { AnswerStyle } from "../generated/AnswerStyle";
import type { CallType } from "../generated/CallType";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { LayoutMode } from "../generated/LayoutMode";
import type { SessionId } from "../generated/SessionId";
import type { SettingsPatch } from "../generated/SettingsPatch";
import type { SettingsView } from "../generated/SettingsView";
import type { CoreApi } from "../ipc/types";
import { ANSWER_FONT, NOTICE, PROMPTER_FONT } from "../state/copy";
import { activeProfile, initialState, isLive, reducer } from "../state/reducer";
import { displayedEntry, selectView } from "../state/select";
import type { Action, State } from "../state/types";
import type { AppActions } from "./view";

export type ScheduleFrame = (cb: () => void) => () => void;

export interface Clock {
  /** Epoch ms (same clock as the core's deadlineMs). */
  now(): number;
  /** Monotonic ms for latency measurement. */
  perf(): number;
}

export interface RecoveryOptions {
  /** get_status polls before giving up. */
  tries: number;
  intervalMs: number;
}

export interface ControllerOptions {
  api: CoreApi;
  /** React's dispatch (the controller mirrors state synchronously). */
  dispatch: (a: Action) => void;
  scheduleFrame?: ScheduleFrame;
  clock?: Clock;
  recovery?: RecoveryOptions;
  /** Retries of the initial get_status while the core answers `unknown`. */
  statusRetries?: { tries: number; intervalMs: number };
  copyText?: (text: string) => Promise<boolean>;
}

export const DEFAULT_RECOVERY: RecoveryOptions = { tries: 5, intervalMs: 400 };

const PROGRESS = new Set(["llm:delta", "llm:done", "session:error", "session:autostopped"]);

/** rAF when visible, with a timer fallback (rAF never fires in hidden tabs). */
export const defaultScheduleFrame: ScheduleFrame = (cb) => {
  let done = false;
  let raf: number | null = null;
  const hidden = typeof document !== "undefined" && document.hidden;
  const run = () => {
    if (done) return;
    done = true;
    cancel();
    cb();
  };
  const cancel = () => {
    if (raf !== null && typeof cancelAnimationFrame === "function") cancelAnimationFrame(raf);
    clearTimeout(timer);
  };
  if (!hidden && typeof requestAnimationFrame === "function") raf = requestAnimationFrame(run);
  const timer = setTimeout(run, raf === null ? 16 : 100);
  return () => {
    done = true;
    cancel();
  };
};

const defaultClock: Clock = {
  now: () => Date.now(),
  perf: () => (typeof performance !== "undefined" ? performance.now() : Date.now()),
};

/** navigator.clipboard with a textarea + execCommand fallback. */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the legacy path
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = typeof document.execCommand === "function" && document.execCommand("copy");
    document.body.removeChild(ta);
    return !!ok;
  } catch {
    return false;
  }
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const sleep = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));

export interface Controller {
  getState(): State;
  actions: AppActions;
  /** Subscribe to events FIRST, then (once per controller) fetch status + settings. */
  attach(): () => void;
  syncCloseGuard(active: boolean): void;
  /** Exposed for tests. */
  onEnvelope(env: EventEnvelope): void;
}

export function createController(opts: ControllerOptions): Controller {
  const { api } = opts;
  const clock = opts.clock ?? defaultClock;
  const scheduleFrame = opts.scheduleFrame ?? defaultScheduleFrame;
  const recoveryOpts = opts.recovery ?? DEFAULT_RECOVERY;
  const statusRetries = opts.statusRetries ?? { tries: 3, intervalMs: 500 };
  const copyText = opts.copyText ?? copyToClipboard;

  let state = initialState();
  const dispatch = (a: Action) => {
    state = reducer(state, a);
    opts.dispatch(a);
  };
  const getState = () => state;

  // ── event intake ──
  let seenSeq = 0;
  let queue: EventEnvelope[] = [];
  let cancelFrame: (() => void) | null = null;
  let initialized = false;
  let closeGuard = false;

  const flush = () => {
    if (cancelFrame) {
      cancelFrame();
      cancelFrame = null;
    }
    if (queue.length === 0) return;
    const envelopes = queue;
    queue = [];
    dispatch({ type: "events", envelopes, now: clock.now() });
  };

  function onEnvelope(env: EventEnvelope) {
    if (!env || typeof env.seq !== "number" || env.seq <= seenSeq) return; // duplicates / replays
    seenSeq = env.seq;
    if ("sessionId" in env && recovery && recovery.sessionId === env.sessionId && PROGRESS.has(env.type)) {
      recovery = null; // progress for X: the core is handling it
    }
    queue.push(env);
    if (env.type === "llm:delta") {
      // Coalesce: at most one dispatch (paint) per animation frame.
      if (!cancelFrame) cancelFrame = scheduleFrame(flush);
      return;
    }
    flush(); // everything else applies immediately, in order, behind any buffered deltas
    switch (env.type) {
      case "hotkey:toggle":
        onHotkey();
        break;
      case "settings:changed":
        void refreshSettings();
        break;
      case "core:ready":
        void refreshSettings();
        void refreshStatus(0);
        break;
      case "window:close-requested":
        // Ack the cancelled close: a responsive page keeps its draft guarded
        // (the shell treats any command as a ping; this is the explicit one).
        if (closeGuard) void api.setCloseGuard(true);
        break;
      default:
        break;
    }
  }

  function onHotkey() {
    const s = state.session;
    if (state.screen === "settings") {
      // Settings open: the hotkey may only STOP, never start.
      if (s && (s.phase === "starting" || s.phase === "recording")) stop();
      return;
    }
    toggleRecord();
  }

  async function refreshSettings(): Promise<SettingsView | null> {
    const r = await api.getSettings();
    if (r.ok) {
      dispatch({ type: "settings/adopt", settings: r.value });
      return r.value;
    }
    return null;
  }

  async function refreshStatus(retries: number): Promise<void> {
    for (let i = 0; ; i++) {
      const r = await api.getStatus();
      if (r.ok) {
        dispatch({ type: "status/snapshot", snapshot: r.value });
        if (r.value.session.phase !== "unknown" || i >= retries) return;
      } else if (i >= retries) return;
      await sleep(statusRetries.intervalMs);
    }
  }

  // ── refused-stop recovery (§14.3) ──
  let recovery: { sessionId: SessionId; token: number } | null = null;
  let recoveryToken = 0;
  const recoveryActive = (token: number) => recovery !== null && recovery.token === token;

  function armRecovery(id: SessionId) {
    if (state.session?.id !== id || !isLive(state.session)) return;
    const token = ++recoveryToken;
    recovery = { sessionId: id, token };
    void runRecovery(id, token);
  }

  function giveUp(id: SessionId) {
    recovery = null;
    dispatch({ type: "recovery/gone", sessionId: id, notice: NOTICE.stopNotTaken });
    void api.cancelSession(id);
  }

  async function runRecovery(id: SessionId, token: number) {
    for (let i = 0; i < recoveryOpts.tries; i++) {
      await sleep(recoveryOpts.intervalMs);
      if (!recoveryActive(token)) return;
      const r = await api.getStatus();
      if (!recoveryActive(token)) return;
      if (!r.ok) continue;
      dispatch({ type: "status/snapshot", snapshot: r.value });
      const ss = r.value.session;
      if (ss.phase === "unknown") continue; // core busy: wait and ask again (bounded)
      if (ss.id === id && (ss.phase === "starting" || ss.phase === "recording")) {
        const sr = await api.stopSession(id);
        if (!recoveryActive(token)) return;
        if (sr.ok) {
          recovery = null;
          dispatch({ type: "cmd/stop-result", sessionId: id, result: sr });
          return;
        }
        continue;
      }
      if (ss.id === id && (ss.phase === "finalizing" || ss.phase === "answering")) {
        recovery = null;
        dispatch({ type: "recovery/phase", sessionId: id, phase: ss.phase });
        return;
      }
      giveUp(id); // X is gone / idle on the core
      return;
    }
    if (recoveryActive(token)) giveUp(id);
  }

  // ── commands ──
  const nextGen = () => state.cmdGen + 1;

  async function doStop(id: SessionId) {
    const r = await api.stopSession(id);
    dispatch({ type: "cmd/stop-result", sessionId: id, result: r });
    if (!r.ok) armRecovery(id);
  }

  function record() {
    const phase = state.session && isLive(state.session) ? state.session.phase : "idle";
    if (state.core !== "ready" || (phase !== "idle" && phase !== "answering")) return;
    const gen = nextGen();
    dispatch({ type: "cmd/start", gen });
    void (async () => {
      const r = await api.startSession();
      dispatch({ type: "cmd/start-result", gen, result: r });
      if (!r.ok) return;
      recovery = null;
      const cur = state.session;
      if (!cur || cur.gen !== gen || cur.id !== r.value || !isLive(cur)) {
        void api.cancelSession(r.value); // lost the race: tear down only what we created
        return;
      }
      if (cur.stopQueued) void doStop(r.value);
    })();
  }

  function stop() {
    const s = state.session;
    if (!s || (s.phase !== "starting" && s.phase !== "recording") || s.stopPending) return;
    dispatch({ type: "cmd/stop", gen: nextGen(), clickedAtMs: clock.perf() });
    if (s.id !== null) void doStop(s.id); // else queued until the start result arrives
  }

  function toggleRecord() {
    const s = state.session;
    if (s && (s.phase === "starting" || s.phase === "recording")) stop();
    else record(); // no-op while finalizing
  }

  function ask(text: string) {
    const q = text.trim();
    if (!q) {
      dispatch({ type: "notice", notice: NOTICE.emptyQuestion });
      return;
    }
    if (state.core !== "ready") return;
    const gen = nextGen();
    dispatch({ type: "cmd/ask", gen, question: q, clickedAtMs: clock.perf() });
    void (async () => {
      const r = await api.ask(q);
      dispatch({ type: "cmd/ask-result", gen, result: r });
      if (!r.ok) return;
      recovery = null;
      const cur = state.session;
      if (!cur || cur.gen !== gen || cur.id !== r.value || !isLive(cur)) void api.cancelSession(r.value);
    })();
  }

  function cancel() {
    const s = state.session;
    dispatch({ type: "cmd/cancel", gen: nextGen() });
    if (s && s.id !== null && isLive(s)) {
      if (recovery?.sessionId === s.id) recovery = null;
      void api.cancelSession(s.id);
    }
  }

  // ── settings (serialized saves; baseRevision read when each save runs) ──
  let saveChain: Promise<unknown> = Promise.resolve();
  function enqueue<T>(fn: () => Promise<T>): Promise<T> {
    const p = saveChain.then(fn, fn);
    saveChain = p.catch(() => undefined);
    return p;
  }

  type SaveResult = { ok: true } | { ok: false; message: string; stale: boolean };

  function saveSettings(patch: Omit<SettingsPatch, "baseRevision">): Promise<SaveResult> {
    return enqueue(async (): Promise<SaveResult> => {
      const base = state.settings?.settingsRevision;
      if (base === undefined) return { ok: false, message: "Settings have not loaded yet — try again in a moment.", stale: false };
      const r = await api.setSettings({ ...patch, baseRevision: base });
      if (r.ok) {
        dispatch({ type: "settings/adopt", settings: r.value });
        return { ok: true };
      }
      // Re-fetch so the next save uses the fresh revision; the form keeps its draft.
      const fresh = await refreshSettings();
      const stale = (fresh !== null && fresh.settingsRevision !== base) || /stale|revision/i.test(r.error.message);
      return { ok: false, message: stale ? NOTICE.staleSave : r.error.message, stale };
    });
  }

  function quickSave(patch: Omit<SettingsPatch, "baseRevision">) {
    if (!state.settings) return;
    dispatch({ type: "settings/optimistic", patch: patch as Partial<SettingsView> });
    void saveSettings(patch).then((r) => {
      if (!r.ok) dispatch({ type: "notice", notice: r.message });
    });
  }

  function bumpFont(key: "answerFontPx" | "prompterFontPx", delta: 1 | -1) {
    const s = state.settings;
    if (!s) return;
    const lim = key === "answerFontPx" ? ANSWER_FONT : PROMPTER_FONT;
    const next = clamp(s[key] + delta * lim.step, lim.min, lim.max);
    if (next !== s[key]) quickSave({ [key]: next });
  }

  const actions: AppActions = {
    record,
    stop,
    toggleRecord,
    ask,
    cancel,
    regenerate() {
      if (!selectView(state, clock.now()).canRegenerate) return;
      const entry = displayedEntry(state);
      if (entry) ask(entry.question);
    },
    historyPrev: () => dispatch({ type: "history/prev" }),
    historyNext: () => dispatch({ type: "history/next" }),
    clearHistory: () => dispatch({ type: "history/clear" }),
    setStyle: (style: AnswerStyle) => quickSave({ answerStyle: style }),
    setCallType(callType: CallType) {
      const s = state.settings;
      const p = activeProfile(s);
      if (!s || !p || p.callType === callType) return;
      quickSave({ profiles: s.profiles.map((x) => (x.id === p.id ? { ...x, callType } : x)) });
    },
    setActiveProfile(profileId: string) {
      const s = state.settings;
      if (!s || s.activeProfileId === profileId || !s.profiles.some((p) => p.id === profileId)) return;
      quickSave({ activeProfileId: profileId });
    },
    setLayout(layout: LayoutMode) {
      if (!state.settings || state.settings.layoutMode === layout) return;
      quickSave({ layoutMode: layout });
    },
    bumpAnswerFont: (d) => bumpFont("answerFontPx", d),
    bumpPrompterFont: (d) => bumpFont("prompterFontPx", d),
    dock: () => void api.dockWindow(),
    openSettings: () => dispatch({ type: "screen", screen: "settings" }),
    closeSettings: () => dispatch({ type: "screen", screen: "main" }),
    saveSettings,
    setSettingsDirty: (dirty) => dispatch({ type: "settings/dirty", dirty }),
    dismissCloseRequest: () => dispatch({ type: "close/dismiss" }),
    openExternal(url) {
      void api.openExternal(url).then((r) => {
        if (!r.ok) dispatch({ type: "notice", notice: r.error.message });
      });
    },
    async copyDiagnostics() {
      const r = await api.getDiagnostics();
      if (!r.ok) {
        dispatch({ type: "notice", notice: r.error.message });
        return null;
      }
      if (!(await copyText(r.value))) {
        dispatch({ type: "notice", notice: NOTICE.copyFailed });
        return null;
      }
      return r.value;
    },
    dismissNotice: () => dispatch({ type: "notice", notice: null }),
    reportFirstPaint: () => dispatch({ type: "first-paint", atMs: clock.perf() }),
  };

  return {
    getState,
    actions,
    onEnvelope,
    attach() {
      const unsubscribe = api.subscribe(onEnvelope);
      if (!initialized) {
        initialized = true;
        void refreshStatus(statusRetries.tries);
        void refreshSettings();
      }
      return () => {
        unsubscribe();
        flush();
        recovery = null;
      };
    },
    syncCloseGuard(active: boolean) {
      if (active === closeGuard) return;
      closeGuard = active;
      void api.setCloseGuard(active);
    },
  };
}

// PURE reducer for the page. No I/O, no clocks: every time value arrives on
// the action. Side effects (commands, recovery, coalescing) live in
// src/app/AppProvider.tsx.
import type { CallType } from "../generated/CallType";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { Profile } from "../generated/Profile";
import type { SessionId } from "../generated/SessionId";
import type { SettingsView } from "../generated/SettingsView";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import { HISTORY_CAP, LOUD_RMS } from "./copy";
import type { Action, HistoryEntry, LiveSession, State } from "./types";

const PENDING_CAP = 256;
const ENDED_CAP = 32;

export function initialState(): State {
  return {
    core: "starting",
    coreError: null,
    protection: "unknown",
    revision: -1,
    settings: null,
    screen: "main",
    settingsDirty: false,
    closeRequested: false,
    session: null,
    history: [],
    historyIndex: null,
    cmdGen: 0,
    lastSeq: 0,
    pending: [],
    endedIds: [],
    notice: null,
  };
}

export function activeProfile(settings: SettingsView | null): Profile | null {
  if (!settings) return null;
  return settings.profiles.find((p) => p.id === settings.activeProfileId) ?? settings.profiles[0] ?? null;
}

function activeCallType(state: State): CallType {
  return activeProfile(state.settings)?.callType ?? "behavioral";
}

export function isLive(s: LiveSession | null): boolean {
  return !!s && s.phase !== "idle";
}

function newSession(gen: number, kind: LiveSession["kind"], callType: CallType): LiveSession {
  return {
    id: null,
    gen,
    kind,
    phase: kind === "ask" ? "answering" : "starting",
    deadlineMs: null,
    capMs: null,
    startedAtMs: null,
    transcript: "",
    level: 0,
    lastLoudAtMs: null,
    answer: "",
    streaming: false,
    finish: null,
    metrics: null,
    error: null,
    callType,
    autostopped: false,
    deviceLost: false,
    stopPending: false,
    stopQueued: false,
    stopClickedAtMs: null,
    visibleFirstWordMs: null,
    pushed: false,
  };
}

function addEnded(ids: SessionId[], id: SessionId | null): SessionId[] {
  if (id === null || ids.includes(id)) return ids;
  const next = [...ids, id];
  return next.length > ENDED_CAP ? next.slice(next.length - ENDED_CAP) : next;
}

/** Replace the session slot with a new command's session (§5.1 supersede). */
function replaceSession(state: State, session: LiveSession): State {
  const old = state.session;
  // Answered entries are already in history; a superseded in-progress entry
  // is dropped. The view jumps back to live.
  return {
    ...state,
    session,
    cmdGen: Math.max(state.cmdGen, session.gen),
    pending: [],
    historyIndex: null,
    endedIds: old ? addEnded(state.endedIds, old.id) : state.endedIds,
  };
}

function toHistoryEntry(s: LiveSession): HistoryEntry {
  return {
    sessionId: s.id,
    question: s.transcript,
    answer: s.answer,
    finish: s.finish,
    callType: s.callType,
    metrics: s.metrics,
    visibleFirstWordMs: s.visibleFirstWordMs,
    error: s.error,
  };
}

/** Terminal: session → idle; push to history when there is something to show. */
function finishSession(state: State, s: LiveSession): State {
  const done: LiveSession = { ...s, phase: "idle", streaming: false, stopPending: false, stopQueued: false };
  let history = state.history;
  let historyIndex = state.historyIndex;
  const worth = done.transcript.trim() !== "" || done.answer.trim() !== "";
  if (worth && !done.pushed) {
    done.pushed = true;
    history = [...history, toHistoryEntry(done)];
    if (history.length > HISTORY_CAP) {
      const drop = history.length - HISTORY_CAP;
      history = history.slice(drop);
      if (historyIndex !== null) historyIndex = Math.max(0, historyIndex - drop);
    }
  }
  return { ...state, session: done, history, historyIndex, endedIds: addEnded(state.endedIds, done.id) };
}

// ───────────────────────────── events ─────────────────────────────

type SessionEnvelope = Extract<EventEnvelope, { sessionId: SessionId }>;

function isSessionEvent(env: EventEnvelope): env is SessionEnvelope {
  return "sessionId" in env && typeof (env as { sessionId?: unknown }).sessionId === "string";
}

function applySessionEvent(state: State, env: SessionEnvelope, now: number): State {
  const s = state.session;
  if (!s || s.id !== env.sessionId) return state;
  // At most one terminal per session; nothing mutates a finished/cancelled entry.
  if (s.pushed || s.phase === "idle") return state;
  switch (env.type) {
    case "session:recording": {
      const phase = s.phase === "starting" || s.phase === "recording" ? "recording" : s.phase;
      return {
        ...state,
        session: { ...s, phase, deadlineMs: env.deadlineMs, capMs: env.capMs, startedAtMs: env.deadlineMs - env.capMs },
      };
    }
    case "stt:partial":
      return { ...state, session: { ...s, transcript: env.text } };
    case "audio:level": {
      if (s.phase !== "recording" && s.phase !== "starting") return state;
      const rms = Number.isFinite(env.rms) ? Math.min(1, Math.max(0, env.rms)) : 0;
      return {
        ...state,
        session: { ...s, level: rms, lastLoudAtMs: rms >= LOUD_RMS ? now : s.lastLoudAtMs },
      };
    }
    case "audio:device": {
      const lost = env.kind === "lost";
      const toFinalize = lost && (s.phase === "starting" || s.phase === "recording");
      return {
        ...state,
        notice: env.message,
        session: {
          ...s,
          deviceLost: s.deviceLost || lost,
          phase: toFinalize ? "finalizing" : s.phase,
          stopPending: toFinalize ? false : s.stopPending,
        },
      };
    }
    case "session:autostopped": {
      const toFinalize = s.phase === "starting" || s.phase === "recording";
      return {
        ...state,
        session: {
          ...s,
          autostopped: true,
          phase: toFinalize ? "finalizing" : s.phase,
          stopPending: toFinalize ? false : s.stopPending,
        },
      };
    }
    case "llm:delta":
      return {
        ...state,
        session: { ...s, answer: s.answer + env.delta, streaming: true, phase: "answering", stopPending: false },
      };
    case "llm:done":
      return finishSession(state, {
        ...s,
        answer: env.answer,
        transcript: env.transcript || s.transcript,
        finish: env.finish,
        callType: env.callType,
        metrics: env.metrics,
        error: null,
      });
    case "session:error":
      return finishSession(state, { ...s, error: env.error });
  }
}

function applyEnvelope(state: State, env: EventEnvelope, now: number): State {
  if (typeof env.seq !== "number" || env.seq <= state.lastSeq) return state;
  const st: State = { ...state, lastSeq: env.seq };
  if (isSessionEvent(env)) {
    const s = st.session;
    if (s && s.id === null && isLive(s)) {
      // Our start/ask is still in flight: we can't tell yet whether this is ours.
      const pending = [...st.pending, { env, now }];
      return { ...st, pending: pending.length > PENDING_CAP ? pending.slice(pending.length - PENDING_CAP) : pending };
    }
    return applySessionEvent(st, env, now);
  }
  switch (env.type) {
    case "protection:ok":
    case "protection:failed":
      if (env.revision < st.revision) return st;
      return { ...st, revision: env.revision, protection: env.protection };
    case "core:ready":
      if (env.revision < st.revision) return st;
      return { ...st, revision: env.revision, core: "ready", coreError: null };
    case "core:failed":
      if (env.revision < st.revision) return st;
      return { ...st, revision: env.revision, core: "failed", coreError: env.error };
    case "window:close-requested":
      return { ...st, closeRequested: true };
    case "hotkey:toggle":
    case "settings:changed":
      // Side effects only (AppProvider).
      return st;
    default:
      return st;
  }
}

// ───────────────────────────── snapshots ─────────────────────────────

const LIVE_CORE_PHASES = new Set(["starting", "recording", "finalizing", "answering"]);

function adoptSnapshot(state: State, snap: StatusSnapshot): State {
  if (snap.revision < state.revision) return state;
  let st: State = {
    ...state,
    revision: snap.revision,
    core: snap.core,
    coreError: snap.coreError,
    protection: snap.protection,
  };
  const ss = snap.session;
  if (ss.id === null || !LIVE_CORE_PHASES.has(ss.phase) || st.endedIds.includes(ss.id)) return st;
  const phase = ss.phase as LiveSession["phase"];
  const cur = st.session;
  if (!cur || (cur.phase === "idle" && cur.id !== ss.id)) {
    // Page load / reload: re-adopt the live session including its deadline.
    const s = newSession(st.cmdGen, "adopted", activeCallType(st));
    s.id = ss.id;
    s.phase = phase;
    if (ss.recording) {
      s.deadlineMs = ss.recording.deadlineMs;
      s.capMs = ss.recording.capMs;
      s.startedAtMs = ss.recording.deadlineMs - ss.recording.capMs;
    }
    st = { ...st, session: s, historyIndex: null };
  } else if (cur.id === ss.id && isLive(cur)) {
    let s = cur;
    if (ss.recording && s.deadlineMs === null) {
      s = {
        ...s,
        deadlineMs: ss.recording.deadlineMs,
        capMs: ss.recording.capMs,
        startedAtMs: ss.recording.deadlineMs - ss.recording.capMs,
      };
    }
    if (s.phase === "starting" && phase === "recording" && s.deadlineMs !== null) s = { ...s, phase: "recording" };
    if (s !== cur) st = { ...st, session: s };
  }
  return st;
}

// ───────────────────────────── history ─────────────────────────────

/** Items the history arrows walk: history + the live, not-yet-pushed session. */
export function navItemCount(state: State): number {
  const s = state.session;
  const liveExtra = s && !s.pushed && (isLive(s) || s.transcript !== "" || s.answer !== "") ? 1 : 0;
  return state.history.length + liveExtra;
}

// ───────────────────────────── reducer ─────────────────────────────

export function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "events": {
      let st = state;
      for (const env of action.envelopes) st = applyEnvelope(st, env, action.now);
      return st;
    }
    case "status/snapshot":
      return adoptSnapshot(state, action.snapshot);
    case "settings/adopt": {
      const cur = state.settings;
      if (cur && action.settings.settingsRevision < cur.settingsRevision) return state;
      return { ...state, settings: action.settings };
    }
    case "settings/optimistic":
      if (!state.settings) return state;
      return { ...state, settings: { ...state.settings, ...action.patch, settingsRevision: state.settings.settingsRevision } };
    case "cmd/start":
      return replaceSession({ ...state, notice: null }, newSession(action.gen, "record", activeCallType(state)));
    case "cmd/ask": {
      const s = newSession(action.gen, "ask", activeCallType(state));
      s.transcript = action.question;
      s.stopClickedAtMs = action.clickedAtMs;
      return replaceSession({ ...state, notice: null }, s);
    }
    case "cmd/start-result":
    case "cmd/ask-result": {
      const s = state.session;
      // Only the command that created the current slot may touch it (§14.2).
      // A cancelled slot (idle, id unknown) ignores its late result; the
      // provider cancels the orphaned core session.
      if (!s || s.gen !== action.gen || s.id !== null || !isLive(s)) return state;
      if (!action.result.ok) {
        return {
          ...state,
          pending: [],
          session: { ...s, phase: "idle", streaming: false, stopPending: false, stopQueued: false, error: action.result.error },
        };
      }
      const id = action.result.value;
      if (state.endedIds.includes(id)) return state;
      let st: State = { ...state, session: { ...s, id }, pending: [] };
      for (const p of state.pending) {
        if (isSessionEvent(p.env) && p.env.sessionId === id) st = applySessionEvent(st, p.env, p.now);
      }
      return st;
    }
    case "cmd/stop": {
      const s = state.session;
      const cmdGen = Math.max(state.cmdGen, action.gen);
      if (!s || (s.phase !== "starting" && s.phase !== "recording") || s.stopPending) return { ...state, cmdGen };
      return {
        ...state,
        cmdGen,
        session: { ...s, stopPending: true, stopQueued: s.id === null, stopClickedAtMs: action.clickedAtMs },
      };
    }
    case "cmd/stop-result": {
      const s = state.session;
      if (!s || s.id !== action.sessionId) return state;
      if (action.result.ok) {
        const toFinalize = s.phase === "starting" || s.phase === "recording";
        return { ...state, session: { ...s, stopPending: false, stopQueued: false, phase: toFinalize ? "finalizing" : s.phase } };
      }
      // Refused: the recovery loop (AppProvider) decides what really happened.
      return { ...state, session: { ...s, stopPending: false, stopQueued: false } };
    }
    case "cmd/cancel": {
      const s = state.session;
      const cmdGen = Math.max(state.cmdGen, action.gen);
      if (!s || !isLive(s)) return { ...state, cmdGen };
      return {
        ...state,
        cmdGen,
        pending: [],
        endedIds: addEnded(state.endedIds, s.id),
        session: { ...s, phase: "idle", streaming: false, stopPending: false, stopQueued: false },
      };
    }
    case "recovery/phase": {
      const s = state.session;
      if (!s || s.id !== action.sessionId || !isLive(s)) return state;
      if (s.phase === "starting" || s.phase === "recording" || (s.phase === "finalizing" && action.phase === "answering")) {
        return { ...state, session: { ...s, phase: action.phase, stopPending: false } };
      }
      return state;
    }
    case "recovery/gone": {
      const s = state.session;
      if (!s || s.id !== action.sessionId || !isLive(s)) return state;
      const st = finishSession(state, s);
      return { ...st, notice: action.notice ?? st.notice };
    }
    case "history/prev": {
      const n = navItemCount(state);
      const cur = state.historyIndex ?? n - 1;
      if (cur <= 0) return state;
      return { ...state, historyIndex: cur - 1 };
    }
    case "history/next": {
      if (state.historyIndex === null) return state;
      const n = navItemCount(state);
      const next = state.historyIndex + 1;
      return { ...state, historyIndex: next >= n - 1 ? null : next };
    }
    case "history/clear": {
      const s = state.session;
      return { ...state, history: [], historyIndex: null, session: s && !isLive(s) ? null : s };
    }
    case "screen":
      return {
        ...state,
        screen: action.screen,
        settingsDirty: action.screen === "main" ? false : state.settingsDirty,
      };
    case "settings/dirty":
      return state.settingsDirty === action.dirty ? state : { ...state, settingsDirty: action.dirty };
    case "close/requested":
      return { ...state, closeRequested: true };
    case "close/dismiss":
      return { ...state, closeRequested: false };
    case "notice":
      return { ...state, notice: action.notice };
    case "first-paint": {
      const s = state.session;
      if (!s || s.stopClickedAtMs === null || s.visibleFirstWordMs !== null || s.answer === "") return state;
      const ms = Math.max(0, Math.round(action.atMs - s.stopClickedAtMs));
      const history = s.pushed
        ? state.history.map((h) => (h.sessionId === s.id && h.visibleFirstWordMs === null ? { ...h, visibleFirstWordMs: ms } : h))
        : state.history;
      return { ...state, history, session: { ...s, visibleFirstWordMs: ms } };
    }
  }
}

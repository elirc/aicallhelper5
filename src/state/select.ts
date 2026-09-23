// selectView(state, now) -> AppView. Pure; `now` is epoch ms (Date.now()).
import type { AnswerEntryView, AppView, RecordingView } from "../app/view";
import type { SettingsView } from "../generated/SettingsView";
import { HOTKEY, SILENCE_MS, STATUS, recordingStatus } from "./copy";
import { activeProfile, isLive, navItemCount } from "./reducer";
import type { HistoryEntry, LiveSession, State } from "./types";

export function needsSetup(settings: SettingsView | null): boolean {
  if (!settings) return false;
  const hasKey = (id: string) => settings.keys.some((k) => k.id === id && k.hasKey);
  if (!hasKey("deepgram")) return true;
  const provider = settings.providers.find((p) => p.id === settings.llmProvider);
  if (!provider) return false;
  return !hasKey(provider.keyId);
}

export function isSilent(s: LiveSession | null, now: number): boolean {
  if (!s || s.phase !== "recording" || s.startedAtMs === null) return false;
  const since = Math.max(s.startedAtMs, s.lastLoudAtMs ?? -Infinity);
  return now - since >= SILENCE_MS;
}

export function statusText(state: State, now: number): string {
  if (state.core === "starting") return STATUS.coreStarting;
  if (state.core === "failed") return state.coreError?.message || STATUS.coreFailed;
  const s = state.session;
  switch (s && isLive(s) ? s.phase : "idle") {
    case "starting":
      return STATUS.starting;
    case "recording":
      return recordingStatus(s?.startedAtMs != null ? now - s.startedAtMs : 0);
    case "finalizing":
      return s?.autostopped && !s.deviceLost ? STATUS.capReached : STATUS.finalizing;
    case "answering":
      return STATUS.answering;
    default:
      return needsSetup(state.settings) ? STATUS.needsSetup : STATUS.ready;
  }
}

export function hotkeyHint(settings: SettingsView | null): string {
  if (!settings) return "";
  switch (settings.hotkeyStatus) {
    case "registered":
      return `${settings.hotkey} toggles recording from any app`;
    case "disabled":
      return HOTKEY.disabled;
    case "invalid":
      return settings.hotkeyMessage || HOTKEY.invalid;
    case "unavailable":
      return settings.hotkeyMessage || HOTKEY.unavailable;
  }
}

function liveEntry(s: LiveSession): AnswerEntryView {
  return {
    question: s.transcript,
    answer: s.answer,
    streaming: s.streaming && isLive(s),
    finish: s.finish,
    callType: s.callType,
    metrics: s.metrics,
    visibleFirstWordMs: s.visibleFirstWordMs,
    error: s.error,
  };
}

function historyEntry(h: HistoryEntry): AnswerEntryView {
  return {
    question: h.question,
    answer: h.answer,
    streaming: false,
    finish: h.finish,
    callType: h.callType,
    metrics: h.metrics,
    visibleFirstWordMs: h.visibleFirstWordMs,
    error: h.error,
  };
}

/** The entry currently displayed (live session, or a history entry). */
export function displayedEntry(state: State): AnswerEntryView | null {
  if (state.historyIndex !== null) {
    const h = state.history[state.historyIndex];
    if (h) return historyEntry(h);
    // Index points at the live, not-yet-pushed item.
  }
  return state.session ? liveEntry(state.session) : null;
}

export function selectView(state: State, now: number): AppView {
  const s = state.session;
  const settings = state.settings;
  const phase = s && isLive(s) ? s.phase : "idle";
  const coreReady = state.core === "ready";
  const entry = displayedEntry(state);

  let recording: RecordingView | null = null;
  if (s && phase === "recording" && s.deadlineMs !== null && s.capMs !== null && s.startedAtMs !== null) {
    recording = {
      deadlineMs: s.deadlineMs,
      capMs: s.capMs,
      startedAtMs: s.startedAtMs,
      level: s.level,
      silent: isSilent(s, now),
    };
  }

  const n = navItemCount(state);
  const cur = n === 0 ? -1 : (state.historyIndex ?? n - 1);
  const provider = settings?.providers.find((p) => p.id === settings.llmProvider);
  const sessionError = s?.error && s.error.code !== "aborted" ? s.error : null;

  return {
    core: state.core,
    coreError: state.coreError,
    protection: state.protection,
    phase,
    statusText: statusText(state, now),
    recording,
    liveTranscript: s?.transcript ?? "",
    entry,
    history: { index: cur + 1, count: n, canPrev: cur > 0, canNext: state.historyIndex !== null },
    sessionError,
    notice: state.notice,
    settings,
    activeProfile: activeProfile(settings),
    providerName: provider?.displayName ?? settings?.llmProvider ?? "",
    answerStyle: settings?.answerStyle ?? "balanced",
    layout: settings?.layoutMode ?? "full",
    answerFontPx: settings?.answerFontPx ?? 14,
    prompterFontPx: settings?.prompterFontPx ?? 18,
    hotkeyHint: hotkeyHint(settings),
    screen: state.screen,
    needsSetup: needsSetup(settings),
    // A new Record supersedes a streaming answer (§4: start supersedes).
    canRecord: coreReady && (phase === "idle" || phase === "answering"),
    canStop: (phase === "starting" || phase === "recording") && !!s && !s.stopPending,
    canAsk: coreReady,
    canRegenerate: coreReady && phase === "idle" && !!entry && entry.question.trim() !== "",
    settingsDirty: state.settingsDirty,
    closeRequested: state.closeRequested,
  };
}

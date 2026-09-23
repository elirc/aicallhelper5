// PINNED seam between state (src/state, src/ipc, src/app/AppProvider.tsx —
// owned by the "frontend-state" agent) and presentation (src/components,
// src/styles — owned by the "frontend-ui" agent).
//
// Components call `useApp()` and render `view`; they never touch the reducer
// or the IPC directly. `selectView(state)` (frontend-state) derives `AppView`.
// Tests render components inside <AppContext.Provider value={{view, actions}}>
// with a hand-built view (see `makeView()` in src/app/testing.ts).
import { createContext, useContext } from "react";
import type { AnswerStyle } from "../generated/AnswerStyle";
import type { AppError } from "../generated/AppError";
import type { CallType } from "../generated/CallType";
import type { CoreState } from "../generated/CoreState";
import type { Finish } from "../generated/Finish";
import type { LayoutMode } from "../generated/LayoutMode";
import type { Metrics } from "../generated/Metrics";
import type { Profile } from "../generated/Profile";
import type { Protection } from "../generated/Protection";
import type { SettingsPatch } from "../generated/SettingsPatch";
import type { SettingsView } from "../generated/SettingsView";

/** UI phase. `idle` also covers "done" (an answer is shown). */
export type UiPhase = "idle" | "starting" | "recording" | "finalizing" | "answering";

export interface AnswerEntryView {
  /** Question heard / typed. */
  question: string;
  /** Markdown source (streaming or final). */
  answer: string;
  streaming: boolean;
  finish: Finish | null;
  callType: CallType;
  metrics: Metrics | null;
  /** Frontend-measured ms from Stop/Ask click to first painted answer word. */
  visibleFirstWordMs: number | null;
  error: AppError | null;
}

export interface RecordingView {
  /** Epoch ms deadline from the core (single source for the countdown). */
  deadlineMs: number;
  capMs: number;
  /** Epoch ms when recording started (for the elapsed timer). */
  startedAtMs: number;
  /** Latest RMS 0..1. */
  level: number;
  /** True after 5 s with no audible level -> "No call audio detected yet". */
  silent: boolean;
}

export interface AppView {
  core: CoreState;
  coreError: AppError | null;
  protection: Protection;
  phase: UiPhase;
  /** One-line status copy (spec §13 status line), already chosen for phase. */
  statusText: string;
  /** Non-null while recording. */
  recording: RecordingView | null;
  /** Live question transcript for the CURRENT session (may be ""). */
  liveTranscript: string;
  /** The entry currently displayed: live one, or a history entry. */
  entry: AnswerEntryView | null;
  /** History navigation: index is 1-based position shown as "n/m". */
  history: { index: number; count: number; canPrev: boolean; canNext: boolean };
  /** Error for the current/last session (shown with role=alert). */
  sessionError: AppError | null;
  /** Transient notice (device changed, stale-revision save, etc.). */
  notice: string | null;
  settings: SettingsView | null;
  activeProfile: Profile | null;
  /** e.g. "Claude Haiku 4.5 (recommended)". */
  providerName: string;
  answerStyle: AnswerStyle;
  layout: LayoutMode;
  answerFontPx: number;
  prompterFontPx: number;
  /** Hotkey hint text, or why the hotkey is unavailable. */
  hotkeyHint: string;
  screen: "main" | "settings";
  /** Missing keys -> first-run "open Settings…" prompt. */
  needsSetup: boolean;
  /** Record button: enabled when core ready and phase idle. */
  canRecord: boolean;
  /** Stop button: enabled while starting/recording (not finalizing). */
  canStop: boolean;
  canAsk: boolean;
  canRegenerate: boolean;
  /** Settings form has unsaved changes (drives the close guard). */
  settingsDirty: boolean;
  /** Set when the core asked to close while settings were dirty. */
  closeRequested: boolean;
}

export interface AppActions {
  record(): void;
  stop(): void;
  /** Record if idle, stop if recording (button + hotkey). */
  toggleRecord(): void;
  ask(text: string): void;
  cancel(): void;
  regenerate(): void;
  historyPrev(): void;
  historyNext(): void;
  clearHistory(): void;
  setStyle(style: AnswerStyle): void;
  setCallType(callType: CallType): void;
  setActiveProfile(profileId: string): void;
  setLayout(layout: LayoutMode): void;
  bumpAnswerFont(delta: 1 | -1): void;
  bumpPrompterFont(delta: 1 | -1): void;
  dock(): void;
  openSettings(): void;
  closeSettings(): void;
  /** Saves; resolves with the result so the form can show "Saving…" and
   * keep the draft on a stale-revision rejection. `baseRevision` is filled in
   * by the store. */
  saveSettings(patch: Omit<SettingsPatch, "baseRevision">): Promise<{ ok: true } | { ok: false; message: string; stale: boolean }>;
  setSettingsDirty(dirty: boolean): void;
  dismissCloseRequest(): void;
  openExternal(url: string): void;
  copyDiagnostics(): Promise<string | null>;
  dismissNotice(): void;
  /** Called by the answer view when the first answer text is painted. */
  reportFirstPaint(): void;
}

export interface AppContextValue {
  view: AppView;
  actions: AppActions;
}

export const AppContext = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const v = useContext(AppContext);
  if (!v) throw new Error("useApp() outside <AppContext.Provider>");
  return v;
}

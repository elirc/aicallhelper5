import type { AppError } from "../generated/AppError";
import type { CallType } from "../generated/CallType";
import type { CoreState } from "../generated/CoreState";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { Finish } from "../generated/Finish";
import type { Metrics } from "../generated/Metrics";
import type { Protection } from "../generated/Protection";
import type { SessionId } from "../generated/SessionId";
import type { SettingsView } from "../generated/SettingsView";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import type { UiPhase } from "../app/view";
import type { CmdResult } from "../ipc/types";

/** An answered (or failed-with-a-question) entry. View state only: never
 * persisted, never sent anywhere (answers are single-turn, spec §8). */
export interface HistoryEntry {
  sessionId: SessionId | null;
  question: string;
  answer: string;
  finish: Finish | null;
  callType: CallType;
  metrics: Metrics | null;
  visibleFirstWordMs: number | null;
  error: AppError | null;
}

export interface LiveSession {
  /** null while the start/ask command that created it is still in flight. */
  id: SessionId | null;
  /** Command generation that created this slot (§14.2). */
  gen: number;
  kind: "record" | "ask" | "adopted";
  phase: UiPhase;
  /** Epoch ms from the core's `session:recording` (single countdown source). */
  deadlineMs: number | null;
  capMs: number | null;
  /** Epoch ms = deadlineMs - capMs (same clock as the deadline). */
  startedAtMs: number | null;
  transcript: string;
  level: number;
  /** Epoch ms (page clock) of the last level >= LOUD_RMS. */
  lastLoudAtMs: number | null;
  answer: string;
  streaming: boolean;
  finish: Finish | null;
  metrics: Metrics | null;
  error: AppError | null;
  callType: CallType;
  autostopped: boolean;
  deviceLost: boolean;
  /** A stop_session call for this session is in flight. */
  stopPending: boolean;
  /** Stop was clicked before the start result gave us an id. */
  stopQueued: boolean;
  /** performance.now() at the Stop / Ask click (visible-first-word clock). */
  stopClickedAtMs: number | null;
  visibleFirstWordMs: number | null;
  /** Entry already pushed to history (terminal event seen). */
  pushed: boolean;
}

export interface State {
  core: CoreState;
  coreError: AppError | null;
  protection: Protection;
  /** Last adopted core revision (snapshots + core/protection events). */
  revision: number;
  settings: SettingsView | null;
  screen: "main" | "settings";
  settingsDirty: boolean;
  closeRequested: boolean;
  session: LiveSession | null;
  history: HistoryEntry[];
  /** null = live; otherwise index into the nav items list. */
  historyIndex: number | null;
  /** Latest command generation issued (start/ask/stop/cancel). */
  cmdGen: number;
  /** Highest envelope seq applied. Starts at 0 on every page load (the core's
   * seq is per process; a reload re-adopts via get_status). Never reset later. */
  lastSeq: number;
  /** Session events that arrived while a start/ask was in flight (id unknown). */
  pending: Array<{ env: EventEnvelope; now: number }>;
  /** Recently ended / superseded / cancelled ids — never re-adopted. */
  endedIds: SessionId[];
  notice: string | null;
}

export type Action =
  | { type: "events"; envelopes: EventEnvelope[]; now: number }
  | { type: "status/snapshot"; snapshot: StatusSnapshot }
  | { type: "settings/adopt"; settings: SettingsView }
  | { type: "settings/optimistic"; patch: Partial<SettingsView> }
  | { type: "cmd/start"; gen: number }
  | { type: "cmd/ask"; gen: number; question: string; clickedAtMs: number }
  | { type: "cmd/start-result"; gen: number; result: CmdResult<SessionId> }
  | { type: "cmd/ask-result"; gen: number; result: CmdResult<SessionId> }
  | { type: "cmd/stop"; gen: number; clickedAtMs: number }
  | { type: "cmd/stop-result"; sessionId: SessionId; result: CmdResult<null> }
  | { type: "cmd/cancel"; gen: number }
  | { type: "recovery/phase"; sessionId: SessionId; phase: "finalizing" | "answering" }
  | { type: "recovery/gone"; sessionId: SessionId; notice: string | null }
  | { type: "history/prev" }
  | { type: "history/next" }
  | { type: "history/clear" }
  | { type: "screen"; screen: "main" | "settings" }
  | { type: "settings/dirty"; dirty: boolean }
  | { type: "close/requested" }
  | { type: "close/dismiss" }
  | { type: "notice"; notice: string | null }
  | { type: "first-paint"; atMs: number };

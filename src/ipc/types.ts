// PINNED contract for talking to the Rust core. Owned by the orchestrator;
// do not change signatures without coordination. Implementations:
//   src/ipc/tauri.ts  — real (invoke + Channel)
//   src/ipc/fake.ts   — scriptable fake for tests and browser dev
import type { AppError } from "../generated/AppError";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { SessionId } from "../generated/SessionId";
import type { SettingsPatch } from "../generated/SettingsPatch";
import type { SettingsView } from "../generated/SettingsView";
import type { StatusSnapshot } from "../generated/StatusSnapshot";

export type CmdResult<T> = { ok: true; value: T } | { ok: false; error: AppError };

/** Every method resolves (never rejects). A command exceeding 30 s resolves
 * to `{ok:false, error:{code:"internal"}}`. */
export interface CoreApi {
  getSettings(): Promise<CmdResult<SettingsView>>;
  setSettings(patch: SettingsPatch): Promise<CmdResult<SettingsView>>;
  getStatus(): Promise<CmdResult<StatusSnapshot>>;
  startSession(): Promise<CmdResult<SessionId>>;
  stopSession(sessionId: SessionId): Promise<CmdResult<null>>;
  ask(text: string): Promise<CmdResult<SessionId>>;
  cancelSession(sessionId: SessionId): Promise<CmdResult<null>>;
  setCloseGuard(active: boolean): Promise<CmdResult<null>>;
  dockWindow(): Promise<CmdResult<null>>;
  openExternal(url: string): Promise<CmdResult<null>>;
  getDiagnostics(): Promise<CmdResult<string>>;
  /** Subscribe to the ordered core event stream. Returns an unsubscribe fn.
   * Envelopes arrive in `seq` order; the listener must tolerate duplicates
   * (seq <= last seen is dropped by the store). */
  subscribe(listener: (ev: EventEnvelope) => void): () => void;
}

/** Tauri command names (snake_case) and their argument objects. Tauri maps
 * Rust `session_id` <-> JS `sessionId`. */
export const COMMANDS = {
  getSettings: "get_settings",
  setSettings: "set_settings", // { patch }
  getStatus: "get_status",
  startSession: "start_session",
  stopSession: "stop_session", // { sessionId }
  ask: "ask", // { text }
  cancelSession: "cancel_session", // { sessionId }
  setCloseGuard: "set_close_guard", // { active }
  dockWindow: "dock_window",
  openExternal: "open_external", // { url }
  getDiagnostics: "get_diagnostics",
  subscribe: "subscribe_events", // { channel: Channel<EventEnvelope> }
} as const;

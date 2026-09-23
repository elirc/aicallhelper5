// Real CoreApi over Tauri 2: `invoke` for commands, a `Channel` for events.
// Every method RESOLVES (never rejects): thrown errors and a 30 s client-side
// timeout both map to `{ok:false, error:{code:"internal", ...}}`.
import { Channel, invoke } from "@tauri-apps/api/core";
import type { AppError } from "../generated/AppError";
import type { ErrorCode } from "../generated/ErrorCode";
import type { EventEnvelope } from "../generated/EventEnvelope";
import type { SessionId } from "../generated/SessionId";
import type { SettingsPatch } from "../generated/SettingsPatch";
import type { SettingsView } from "../generated/SettingsView";
import type { StatusSnapshot } from "../generated/StatusSnapshot";
import { COMMANDS, type CmdResult, type CoreApi } from "./types";

export const COMMAND_TIMEOUT_MS = 30_000;
export const TIMEOUT_MESSAGE = "The app core did not respond in time.";
export const BAD_RESPONSE_MESSAGE = "The app core sent an unexpected response.";
const SUBSCRIBE_RETRIES = 3;
const SUBSCRIBE_RETRY_MS = 1000;

const ERROR_CODES: ReadonlySet<string> = new Set<ErrorCode>([
  "no_stt_key",
  "no_llm_key",
  "stt_connect",
  "stt_error",
  "stt_timeout",
  "no_speech",
  "llm_auth",
  "llm_http",
  "llm_rate_limit",
  "llm_first_token_timeout",
  "llm_timeout",
  "aborted",
  "internal",
]);

function internal<T>(message: string): CmdResult<T> {
  return { ok: false, error: { code: "internal", message } };
}

function errorMessage(e: unknown): string {
  let msg: string;
  if (typeof e === "string") msg = e;
  else if (e instanceof Error) msg = e.message;
  else if (e && typeof e === "object" && typeof (e as { message?: unknown }).message === "string") {
    msg = (e as { message: string }).message;
  } else msg = "The app core returned an error.";
  msg = msg.trim() || "The app core returned an error.";
  return msg.length > 300 ? `${msg.slice(0, 300)}…` : msg;
}

type Check<T> = (v: unknown) => v is T;

const isNull: Check<null> = (v): v is null => v === null || v === undefined;
const isString: Check<string> = (v): v is string => typeof v === "string";
const isSettings: Check<SettingsView> = (v): v is SettingsView =>
  !!v && typeof v === "object" && typeof (v as SettingsView).settingsRevision === "number" && Array.isArray((v as SettingsView).profiles);
const isStatus: Check<StatusSnapshot> = (v): v is StatusSnapshot =>
  !!v &&
  typeof v === "object" &&
  typeof (v as StatusSnapshot).revision === "number" &&
  typeof (v as StatusSnapshot).core === "string" &&
  !!(v as StatusSnapshot).session &&
  typeof (v as StatusSnapshot).session === "object";

function isAppError(v: unknown): v is AppError {
  return !!v && typeof v === "object" && typeof (v as AppError).code === "string" && typeof (v as AppError).message === "string";
}

/** Validate the CmdResult JSON the Rust side returns as the invoke value. */
export function normalizeResult<T>(raw: unknown, check: Check<T>): CmdResult<T> {
  if (!raw || typeof raw !== "object") return internal(BAD_RESPONSE_MESSAGE);
  const r = raw as { ok?: unknown; value?: unknown; error?: unknown };
  if (r.ok === true) {
    const value = r.value === undefined ? null : r.value;
    return check(value) ? { ok: true, value: value as T } : internal(BAD_RESPONSE_MESSAGE);
  }
  if (r.ok === false) {
    if (!isAppError(r.error)) return internal(BAD_RESPONSE_MESSAGE);
    const code = ERROR_CODES.has(r.error.code) ? (r.error.code as ErrorCode) : "internal";
    return { ok: false, error: { code, message: r.error.message } };
  }
  return internal(BAD_RESPONSE_MESSAGE);
}

export async function call<T>(cmd: string, args: Record<string, unknown> | undefined, check: Check<T>): Promise<CmdResult<T>> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const timeout = new Promise<CmdResult<T>>((resolve) => {
      timer = setTimeout(() => resolve(internal(TIMEOUT_MESSAGE)), COMMAND_TIMEOUT_MS);
    });
    const request = (async () => {
      try {
        const raw = await invoke<unknown>(cmd, args);
        return normalizeResult(raw, check);
      } catch (e) {
        return internal<T>(errorMessage(e));
      }
    })();
    return await Promise.race([request, timeout]);
  } catch (e) {
    return internal(errorMessage(e));
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

function isEnvelope(v: unknown): v is EventEnvelope {
  return !!v && typeof v === "object" && typeof (v as EventEnvelope).seq === "number" && typeof (v as EventEnvelope).type === "string";
}

export function createTauriApi(): CoreApi {
  return {
    getSettings: () => call(COMMANDS.getSettings, undefined, isSettings),
    setSettings: (patch: SettingsPatch) => call(COMMANDS.setSettings, { patch }, isSettings),
    getStatus: () => call(COMMANDS.getStatus, undefined, isStatus),
    startSession: () => call<SessionId>(COMMANDS.startSession, undefined, isString),
    stopSession: (sessionId: SessionId) => call(COMMANDS.stopSession, { sessionId }, isNull),
    ask: (text: string) => call<SessionId>(COMMANDS.ask, { text }, isString),
    cancelSession: (sessionId: SessionId) => call(COMMANDS.cancelSession, { sessionId }, isNull),
    setCloseGuard: (active: boolean) => call(COMMANDS.setCloseGuard, { active }, isNull),
    dockWindow: () => call(COMMANDS.dockWindow, undefined, isNull),
    openExternal: (url: string) => call(COMMANDS.openExternal, { url }, isNull),
    getDiagnostics: () => call(COMMANDS.getDiagnostics, undefined, isString),
    subscribe(listener) {
      let active = true;
      const channel = new Channel<EventEnvelope>();
      channel.onmessage = (msg) => {
        if (!active || !isEnvelope(msg)) return;
        try {
          listener(msg);
        } catch (e) {
          console.error("event listener failed", e);
        }
      };
      // Lesson §14.1: a silently detached event channel breaks everything
      // while every command still works — retry the attach (bounded) and log.
      void (async () => {
        for (let attempt = 1; active && attempt <= SUBSCRIBE_RETRIES; attempt++) {
          const r = await call(COMMANDS.subscribe, { channel }, isNull);
          if (r.ok) return;
          console.error(`subscribe_events failed (attempt ${attempt}): ${r.error.message}`);
          if (attempt < SUBSCRIBE_RETRIES) await new Promise((res) => setTimeout(res, SUBSCRIBE_RETRY_MS));
        }
      })();
      return () => {
        active = false;
      };
    },
  };
}

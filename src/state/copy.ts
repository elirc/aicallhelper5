// User-facing copy owned by the state layer (spec §13 status line).
// NEVER use the word "microphone" here — the app captures system output audio.

export const STATUS = {
  ready: "Ready — press Record while the other person is speaking",
  starting: "Starting system-audio capture…",
  finalizing: "Finalizing transcript…",
  answering: "Generating answer…",
  capReached: "Reached the 120s limit — answering now",
  needsSetup: "Add your API keys — open Settings to get started",
  coreStarting: "Starting…",
  coreFailed:
    "The app core failed to start. Restart the app; if it keeps happening, copy diagnostics from Settings.",
} as const;

/** "Recording — 0:12" */
export function recordingStatus(elapsedMs: number): string {
  return `Recording — ${formatElapsed(elapsedMs)}`;
}

export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export const NOTICE = {
  emptyQuestion: "Type a question first.",
  staleSave:
    "Settings were changed elsewhere, so nothing was saved. Your edits are still here — review them and save again.",
  stopNotTaken: "That recording is no longer active.",
  copyFailed: "Could not copy to the clipboard.",
} as const;

export const HOTKEY = {
  disabled: "Global shortcut is off — set one in Settings",
  invalid: "The global shortcut in Settings is not valid",
  unavailable: "The global shortcut is in use by another app — pick a different one in Settings",
} as const;

/** Silence threshold (RMS) and window for "No call audio detected yet". */
export const LOUD_RMS = 0.01;
export const SILENCE_MS = 5000;
export const HISTORY_CAP = 6;
export const ANSWER_FONT = { min: 12, max: 22, step: 2 } as const;
export const PROMPTER_FONT = { min: 14, max: 28, step: 2 } as const;

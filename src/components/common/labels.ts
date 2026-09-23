import type { AnswerStyle } from "../../generated/AnswerStyle";
import type { CallType } from "../../generated/CallType";
import type { ErrorCode } from "../../generated/ErrorCode";
import type { UiPhase } from "../../app/view";

/** Call types in display order with their labels (spec §8). */
export const CALL_TYPES: ReadonlyArray<{ id: CallType; label: string }> = [
  { id: "behavioral", label: "Behavioral interview" },
  { id: "technical", label: "Technical screen" },
  { id: "system_design", label: "System design" },
  { id: "recruiter", label: "Recruiter screen" },
  { id: "sales", label: "Sales or customer call" },
  { id: "meeting", label: "General meeting" },
];

export function callTypeLabel(id: CallType): string {
  return CALL_TYPES.find((c) => c.id === id)?.label ?? "Behavioral interview";
}

export const STYLES: ReadonlyArray<{ id: AnswerStyle; label: string }> = [
  { id: "brief", label: "Brief" },
  { id: "balanced", label: "Balanced" },
  { id: "detailed", label: "Detailed" },
];

/** Profile field labels switch for sales / meeting call types (spec §8). */
export function profileFieldLabels(callType: CallType): { resume: string; jobDescription: string } {
  if (callType === "sales" || callType === "meeting") {
    return { resume: "Background", jobDescription: "Call context" };
  }
  return { resume: "Resume", jobDescription: "Job description" };
}

/** Error codes whose fix lives in Settings. */
export const SETTINGS_ERROR_CODES: ReadonlySet<ErrorCode> = new Set<ErrorCode>(["no_stt_key", "no_llm_key", "llm_auth"]);

export const ACTIVE_PHASES: ReadonlySet<UiPhase> = new Set<UiPhase>(["starting", "recording", "finalizing", "answering"]);

export const TRUNCATED_NOTE = "The answer was cut off at the length limit.";
export const REFUSED_NOTE = "The model declined to answer this one.";
export const SILENT_NOTE =
  "No call audio detected yet — make sure the call is playing through your default output device.";

import type { AnswerStyle } from "../../generated/AnswerStyle";
import type { Profile } from "../../generated/Profile";
import type { SecretChange } from "../../generated/SecretChange";
import type { SettingsPatch } from "../../generated/SettingsPatch";
import type { SettingsView } from "../../generated/SettingsView";

export const MAX_PROFILES = 20;
export const NAME_MAX = 60;
export const FOCUS_MAX = 2000;
export const TEXT_MAX = 200_000;
export const HOTKEY_MAX = 100;
const PROFILE_ID_RE = /^[A-Za-z0-9_-]{1,64}$/;

/** Per-key secret draft: `value` typed (write-only), `remove` queued. */
export interface SecretDraft {
  value: string;
  remove: boolean;
}

export interface SettingsDraft {
  profiles: Profile[];
  activeProfileId: string;
  llmProvider: string;
  answerStyle: AnswerStyle;
  hotkey: string;
  alwaysOnTop: boolean;
  secrets: Record<string, SecretDraft>;
}

export type DraftPatch = Omit<SettingsPatch, "baseRevision">;

export function draftFromSettings(s: SettingsView): SettingsDraft {
  return {
    profiles: s.profiles.map((p) => ({ ...p })),
    activeProfileId: s.activeProfileId,
    llmProvider: s.llmProvider,
    answerStyle: s.answerStyle,
    hotkey: s.hotkey,
    alwaysOnTop: s.alwaysOnTop,
    secrets: {},
  };
}

function sameProfiles(a: Profile[], b: Profile[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((p, i) => {
    const q = b[i];
    return (
      !!q &&
      p.id === q.id &&
      p.name === q.name &&
      p.callType === q.callType &&
      p.focus === q.focus &&
      p.resume === q.resume &&
      p.jobDescription === q.jobDescription &&
      p.notes === q.notes
    );
  });
}

/**
 * Secret changes: a typed (trimmed, non-empty) value → set; otherwise a
 * queued Remove → remove. An emptied field never removes anything.
 */
export function secretChanges(secrets: Record<string, SecretDraft>): SecretChange[] {
  const out: SecretChange[] = [];
  for (const keyId of Object.keys(secrets).sort()) {
    const d = secrets[keyId];
    if (!d) continue;
    const value = d.value.trim();
    if (value) out.push({ keyId, action: "set", value });
    else if (d.remove) out.push({ keyId, action: "remove" });
  }
  return out;
}

/** Only the fields that differ from `base` (and any secret changes). */
export function buildPatch(base: SettingsView, draft: SettingsDraft): DraftPatch {
  const patch: DraftPatch = {};
  if (!sameProfiles(base.profiles, draft.profiles)) patch.profiles = draft.profiles.map((p) => ({ ...p }));
  if (draft.activeProfileId !== base.activeProfileId) patch.activeProfileId = draft.activeProfileId;
  if (draft.llmProvider !== base.llmProvider) patch.llmProvider = draft.llmProvider;
  if (draft.answerStyle !== base.answerStyle) patch.answerStyle = draft.answerStyle;
  if (draft.hotkey.trim() !== base.hotkey) patch.hotkey = draft.hotkey.trim();
  if (draft.alwaysOnTop !== base.alwaysOnTop) patch.alwaysOnTop = draft.alwaysOnTop;
  const secrets = secretChanges(draft.secrets);
  if (secrets.length) patch.secrets = secrets;
  return patch;
}

export function isDirty(base: SettingsView, draft: SettingsDraft): boolean {
  return Object.keys(buildPatch(base, draft)).length > 0;
}

/** New profile id `p-<base36>` matching the contract regex, unique in `existing`. */
export function newProfileId(existing: ReadonlyArray<{ id: string }>, rand: () => number = Math.random): string {
  const taken = new Set(existing.map((p) => p.id));
  for (let i = 0; i < 100; i++) {
    const id = `p-${Math.floor(rand() * 36 ** 8).toString(36).padStart(4, "0")}`;
    if (PROFILE_ID_RE.test(id) && !taken.has(id)) return id;
  }
  // Practically unreachable; still deterministic and valid.
  let n = existing.length + 1;
  while (taken.has(`p-${n.toString(36)}`)) n++;
  return `p-${n.toString(36)}`;
}

export function duplicateName(name: string): string {
  return `${name} copy`.slice(0, NAME_MAX).trim();
}

const MODIFIERS = new Set(["ctrl", "control", "alt", "shift", "win", "super", "meta", "cmd", "command"]);

/**
 * Client-side shortcut check: empty = disabled (valid); otherwise at least one
 * modifier (Ctrl/Alt/Shift/Win) plus exactly one non-modifier key.
 * Returns null when valid, else the hint to show.
 */
export function hotkeyProblem(hotkey: string): string | null {
  const text = hotkey.trim();
  if (!text) return null;
  if (text.length > HOTKEY_MAX) return `Keep the shortcut under ${HOTKEY_MAX} characters.`;
  const parts = text.split("+").map((p) => p.trim());
  if (parts.some((p) => p === "")) return "Use a shortcut like Ctrl+Shift+Space: modifiers and one key joined by +.";
  const mods = parts.filter((p) => MODIFIERS.has(p.toLowerCase()));
  const keys = parts.filter((p) => !MODIFIERS.has(p.toLowerCase()));
  if (mods.length === 0) return "Add at least one modifier: Ctrl, Alt, Shift or Win.";
  if (keys.length !== 1) return "Use exactly one non-modifier key, e.g. Ctrl+Shift+Space.";
  return null;
}

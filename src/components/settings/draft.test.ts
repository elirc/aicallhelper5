import { describe, expect, it } from "vitest";
import { makeSettings } from "../../app/testing";
import { buildPatch, draftFromSettings, duplicateName, hotkeyProblem, isDirty, newProfileId, secretChanges } from "./draft";

describe("buildPatch", () => {
  it("patch_empty_for_untouched_draft", () => {
    const s = makeSettings();
    expect(buildPatch(s, draftFromSettings(s))).toEqual({});
    expect(isDirty(s, draftFromSettings(s))).toBe(false);
  });

  it("patch_contains_only_changed_fields", () => {
    const s = makeSettings();
    const d = { ...draftFromSettings(s), hotkey: "Ctrl+Alt+K", alwaysOnTop: false };
    expect(buildPatch(s, d)).toEqual({ hotkey: "Ctrl+Alt+K", alwaysOnTop: false });
  });

  it("patch_replaces_whole_profile_list_when_any_profile_changes", () => {
    const s = makeSettings();
    const d = draftFromSettings(s);
    d.profiles = d.profiles.map((p) => ({ ...p, focus: "Rust" }));
    const patch = buildPatch(s, d);
    expect(Object.keys(patch)).toEqual(["profiles"]);
    expect(patch.profiles?.[0]?.focus).toBe("Rust");
  });

  it("patch_draft_is_not_aliased_to_settings", () => {
    const s = makeSettings();
    const d = draftFromSettings(s);
    d.profiles[0]!.name = "Changed";
    expect(s.profiles[0]!.name).toBe("Default");
  });

  it("patch_hotkey_trimmed", () => {
    const s = makeSettings();
    expect(buildPatch(s, { ...draftFromSettings(s), hotkey: "  Ctrl+Shift+Space " })).toEqual({});
  });
});

describe("secretChanges", () => {
  it("secret_typed_value_sets_trimmed", () => {
    expect(secretChanges({ groq: { value: "  gsk_1 ", remove: false } })).toEqual([{ keyId: "groq", action: "set", value: "gsk_1" }]);
  });

  it("secret_queued_remove_removes", () => {
    expect(secretChanges({ anthropic: { value: "", remove: true } })).toEqual([{ keyId: "anthropic", action: "remove" }]);
  });

  it("secret_typed_value_wins_over_remove", () => {
    expect(secretChanges({ anthropic: { value: "sk-new", remove: true } })).toEqual([{ keyId: "anthropic", action: "set", value: "sk-new" }]);
  });

  it("secret_emptied_field_never_removes", () => {
    expect(secretChanges({ anthropic: { value: "   ", remove: false } })).toEqual([]);
  });
});

describe("profile helpers", () => {
  it("new_profile_id_matches_contract_regex_and_is_unique", () => {
    const existing = [{ id: "default" }];
    for (let i = 0; i < 200; i++) {
      const id = newProfileId(existing);
      expect(id).toMatch(/^p-[a-z0-9]+$/);
      expect(id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
      expect(existing.some((p) => p.id === id)).toBe(false);
      existing.push({ id });
    }
  });

  it("new_profile_id_avoids_collisions_with_constant_rng", () => {
    const first = newProfileId([], () => 0.5);
    const second = newProfileId([{ id: first }], () => 0.5);
    expect(second).not.toBe(first);
    expect(second).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
  });

  it("duplicate_name_trimmed_to_60", () => {
    expect(duplicateName("Default")).toBe("Default copy");
    const long = "x".repeat(58);
    expect(duplicateName(long)).toHaveLength(60);
    expect(duplicateName("y".repeat(60))).toBe("y".repeat(60));
  });
});

describe("hotkeyProblem", () => {
  it("hotkey_valid_combos", () => {
    for (const k of ["Ctrl+Shift+Space", "Alt+F9", "Win+K", "Ctrl+Alt+Shift+A", ""]) {
      expect(hotkeyProblem(k)).toBeNull();
    }
  });

  it("hotkey_needs_a_modifier", () => {
    expect(hotkeyProblem("K")).toMatch(/modifier/);
  });

  it("hotkey_needs_exactly_one_key", () => {
    expect(hotkeyProblem("Ctrl+Shift")).toMatch(/one non-modifier key/);
    expect(hotkeyProblem("Ctrl+A+B")).toMatch(/one non-modifier key/);
  });

  it("hotkey_rejects_empty_parts_and_overlong", () => {
    expect(hotkeyProblem("Ctrl++")).not.toBeNull();
    expect(hotkeyProblem(`Ctrl+${"A".repeat(120)}`)).toMatch(/100/);
  });
});

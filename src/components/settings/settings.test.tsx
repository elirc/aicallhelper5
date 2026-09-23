import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { makeSettings } from "../../app/testing";
import type { Profile } from "../../generated/Profile";
import { mocked, renderWithView } from "../testUtils";
import { SettingsScreen, STALE_MESSAGE, hotkeyStatusText } from "./SettingsScreen";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const onSettings = { screen: "settings" as const };

function profile(i: number): Profile {
  return { id: `p-${i}`, name: `Profile ${i}`, callType: "behavioral", focus: "", resume: "", jobDescription: "", notes: "" };
}

function setup(over: Parameters<typeof renderWithView>[1] = {}) {
  const h = renderWithView(<SettingsScreen />, { ...onSettings, ...over });
  mocked(h.actions.saveSettings).mockResolvedValue({ ok: true });
  return h;
}

function lastDirty(h: ReturnType<typeof setup>): boolean | undefined {
  const calls = mocked(h.actions.setSettingsDirty).mock.calls;
  return calls.at(-1)?.[0] as boolean | undefined;
}

describe("API keys", () => {
  it("key_rows_placeholder_and_status", () => {
    setup();
    expect(screen.getByLabelText("Deepgram API key")).toHaveAttribute("placeholder", "saved — type to replace");
    expect(screen.getByLabelText("Anthropic API key")).toHaveAttribute("type", "password");
    expect(screen.getByLabelText("Groq API key")).toHaveAttribute("placeholder", "Paste your key");
    expect(within(screen.getByTestId("key-row-deepgram")).getByText("Saved (encrypted)")).toBeInTheDocument();
    expect(within(screen.getByTestId("key-row-groq")).getByText("Not set")).toBeInTheDocument();
  });

  it("key_row_unreadable_status", () => {
    const base = makeSettings();
    const settings = makeSettings({
      keys: base.keys.map((k) => (k.id === "anthropic" ? { ...k, hasKey: false, storage: "unreadable" as const } : k)),
    });
    setup({ settings });
    expect(within(screen.getByTestId("key-row-anthropic")).getByText("stored key couldn't be read — enter it again")).toBeInTheDocument();
    expect(within(screen.getByTestId("key-row-anthropic")).getByRole("button", { name: "Remove" })).toBeInTheDocument();
  });

  it("key_disclosure_text", () => {
    setup();
    expect(screen.getByText("Keys are encrypted with Windows DPAPI for your user account. Profiles are stored as plain text.")).toBeInTheDocument();
  });

  it("get_a_key_opens_external", async () => {
    const h = setup();
    await userEvent.click(within(screen.getByTestId("key-row-groq")).getByRole("button", { name: "Get a key" }));
    expect(h.actions.openExternal).toHaveBeenCalledWith("https://console.groq.com/keys");
  });

  it("key_remove_queues_remove_in_patch", async () => {
    const h = setup();
    await userEvent.click(within(screen.getByTestId("key-row-anthropic")).getByRole("button", { name: "Remove" }));
    expect(within(screen.getByTestId("key-row-anthropic")).getByText("Will be removed when you save")).toBeInTheDocument();
    expect(lastDirty(h)).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).toHaveBeenCalledWith({ secrets: [{ keyId: "anthropic", action: "remove" }] });
  });

  it("key_typing_cancels_queued_remove", async () => {
    const h = setup();
    const row = screen.getByTestId("key-row-anthropic");
    await userEvent.click(within(row).getByRole("button", { name: "Remove" }));
    await userEvent.type(screen.getByLabelText("Anthropic API key"), "sk-new");
    expect(within(row).getByRole("button", { name: "Remove" })).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).toHaveBeenCalledWith({ secrets: [{ keyId: "anthropic", action: "set", value: "sk-new" }] });
  });

  it("key_emptying_field_does_not_remove", async () => {
    const h = setup();
    const input = screen.getByLabelText("Anthropic API key");
    await userEvent.type(input, "abc");
    expect(lastDirty(h)).toBe(true);
    await userEvent.clear(input);
    expect(lastDirty(h)).toBe(false);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).not.toHaveBeenCalled();
    expect(screen.getByTestId("save-msg")).toHaveTextContent("Nothing to save");
  });

  it("key_undo_remove", async () => {
    const h = setup();
    const row = screen.getByTestId("key-row-deepgram");
    await userEvent.click(within(row).getByRole("button", { name: "Remove" }));
    await userEvent.click(within(row).getByRole("button", { name: "Undo remove" }));
    expect(lastDirty(h)).toBe(false);
  });

  it("key_value_cleared_after_successful_save", async () => {
    setup();
    const input = screen.getByLabelText("Groq API key");
    await userEvent.type(input, "gsk_x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(input).toHaveValue(""));
  });
});

describe("profile editor", () => {
  it("labels_switch_for_sales_and_meeting", async () => {
    setup();
    expect(screen.getByLabelText("Resume")).toBeInTheDocument();
    expect(screen.getByLabelText("Job description")).toBeInTheDocument();
    const section = screen.getByRole("region", { name: "Profiles" });
    await userEvent.selectOptions(within(section).getByLabelText("Call type"), "sales");
    expect(screen.getByLabelText("Background")).toBeInTheDocument();
    expect(screen.getByLabelText("Call context")).toBeInTheDocument();
    await userEvent.selectOptions(within(section).getByLabelText("Call type"), "meeting");
    expect(screen.getByLabelText("Background")).toBeInTheDocument();
    await userEvent.selectOptions(within(section).getByLabelText("Call type"), "technical");
    expect(screen.getByLabelText("Resume")).toBeInTheDocument();
  });

  it("focus_counter_counts", async () => {
    setup();
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    expect(screen.getByTestId("focus-counter")).toHaveTextContent("2/2000");
    expect(screen.getByLabelText("Focus")).toHaveAttribute("maxLength", "2000");
    expect(screen.getByLabelText("Name")).toHaveAttribute("maxLength", "60");
  });

  it("add_profile_appends_and_selects", async () => {
    const h = setup();
    await userEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(screen.getByLabelText("Name")).toHaveValue("New profile");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    const patch = mocked(h.actions.saveSettings).mock.calls[0]![0] as { profiles: Profile[] };
    expect(Object.keys(patch)).toEqual(["profiles"]);
    expect(patch.profiles).toHaveLength(2);
    expect(patch.profiles[1]!.id).toMatch(/^p-[a-z0-9]+$/);
    expect(patch.profiles[1]!.name).toBe("New profile");
  });

  it("duplicate_profile_names_copy", async () => {
    const settings = makeSettings({ profiles: [{ ...profile(1), name: "A".repeat(58), focus: "Kotlin" }], activeProfileId: "p-1" });
    const h = setup({ settings, activeProfile: settings.profiles[0]! });
    await userEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    const name = (screen.getByLabelText("Name") as HTMLInputElement).value;
    expect(name).toHaveLength(60);
    expect(name.startsWith("A".repeat(58))).toBe(true);
    expect(screen.getByLabelText("Focus")).toHaveValue("Kotlin");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    const patch = mocked(h.actions.saveSettings).mock.calls[0]![0] as { profiles: Profile[] };
    expect(patch.profiles[1]!.id).not.toBe("p-1");
  });

  it("delete_disabled_for_last_profile", () => {
    setup();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
  });

  it("delete_active_profile_moves_active_to_first", async () => {
    const settings = makeSettings({ profiles: [profile(1), profile(2)], activeProfileId: "p-2" });
    const h = setup({ settings, activeProfile: settings.profiles[1]! });
    expect(screen.getByLabelText("Editing")).toHaveValue("p-2");
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).toHaveBeenCalledWith({ profiles: [profile(1)], activeProfileId: "p-1" });
  });

  it("add_and_duplicate_disabled_at_20", () => {
    const profiles = Array.from({ length: 20 }, (_, i) => profile(i));
    const settings = makeSettings({ profiles, activeProfileId: "p-0" });
    setup({ settings, activeProfile: profiles[0]! });
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Duplicate" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeEnabled();
  });

  it("use_for_answers_sets_active_profile", async () => {
    const settings = makeSettings({ profiles: [profile(1), profile(2)], activeProfileId: "p-1" });
    const h = setup({ settings, activeProfile: settings.profiles[0]! });
    await userEvent.selectOptions(screen.getByLabelText("Editing"), "p-2");
    await userEvent.click(screen.getByRole("button", { name: "Use for answers" }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).toHaveBeenCalledWith({ activeProfileId: "p-2" });
  });

  it("empty_profile_name_blocks_save", async () => {
    setup();
    await userEvent.clear(screen.getByLabelText("Name"));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByTestId("save-msg")).toHaveTextContent("Every profile needs a name.");
  });
});

describe("general settings", () => {
  it("provider_and_style_patch", async () => {
    const h = setup();
    await userEvent.selectOptions(screen.getByLabelText("Answer provider"), "Groq GPT-OSS 120B (fastest)");
    await userEvent.selectOptions(screen.getByLabelText("Answer style"), "Detailed");
    await userEvent.click(screen.getByLabelText("Keep window on top"));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(h.actions.saveSettings).toHaveBeenCalledWith({ llmProvider: "groq", answerStyle: "detailed", alwaysOnTop: false });
  });

  it("hotkey_validation_hint_blocks_save", async () => {
    setup();
    const input = screen.getByLabelText("Global shortcut");
    await userEvent.clear(input);
    await userEvent.type(input, "K");
    expect(screen.getByTestId("hotkey-hint")).toHaveTextContent("Add at least one modifier");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await userEvent.clear(input);
    await userEvent.type(input, "Ctrl+Alt+K");
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("hotkey_status_text_is_honest", () => {
    const s = makeSettings();
    expect(hotkeyStatusText(s)).toMatch(/^Registered/);
    expect(hotkeyStatusText({ ...s, hotkeyStatus: "disabled", hotkey: "" })).toMatch(/^Disabled/);
    expect(hotkeyStatusText({ ...s, hotkeyStatus: "invalid" })).toMatch(/not valid/);
    expect(hotkeyStatusText({ ...s, hotkeyStatus: "unavailable", hotkeyMessage: "Ctrl+Shift+Space is in use." })).toMatch(
      /another app.*Ctrl\+Shift\+Space is in use\./,
    );
    setup({ settings: makeSettings({ hotkeyStatus: "unavailable", hotkeyRegistered: false, hotkeyMessage: "In use." }) });
    expect(screen.getByTestId("hotkey-status")).toHaveTextContent("Not registered");
    expect(screen.getByTestId("hotkey-status")).toHaveTextContent("In use.");
  });

  it("version_chip_and_single_turn_note", () => {
    setup();
    expect(screen.getByTestId("version-chip")).toHaveTextContent("v4.0.0 · abc1234");
    expect(screen.getByText("Answers are single-turn: previous questions are never sent as context.")).toBeInTheDocument();
  });

  it("copy_diagnostics", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const h = setup();
    mocked(h.actions.copyDiagnostics).mockResolvedValue("version 4.0.0");
    await userEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    expect(await screen.findByText("Diagnostics copied")).toBeInTheDocument();
    expect(h.actions.copyDiagnostics).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith("version 4.0.0");
  });

  it("copy_diagnostics_failure_message", async () => {
    const h = setup();
    mocked(h.actions.copyDiagnostics).mockResolvedValue(null);
    await userEvent.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    expect(await screen.findByText("Couldn't collect diagnostics.")).toBeInTheDocument();
  });

  it("load_issue_banner", () => {
    setup({
      settings: makeSettings({
        loadIssue: { status: "corrupt", backupPath: "C:\\x\\settings.json.corrupt-1.bak", writesBlocked: false, message: "Your settings file was damaged; defaults loaded." },
      }),
    });
    const b = screen.getByTestId("load-issue");
    expect(b).toHaveTextContent("Your settings file was damaged; defaults loaded.");
    expect(b).toHaveTextContent("settings.json.corrupt-1.bak");
    expect(within(b).queryByRole("alert")).toBeNull();
  });

  it("load_issue_writes_blocked_alert_and_save_disabled", async () => {
    setup({
      settings: makeSettings({
        loadIssue: { status: "unreadable", backupPath: null, writesBlocked: true, message: "Settings could not be read." },
      }),
    });
    expect(within(screen.getByTestId("load-issue")).getByRole("alert")).toHaveTextContent("Settings can't be saved");
    await userEvent.type(screen.getByLabelText("Focus"), "x");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });
});

describe("saving", () => {
  it("saving_disables_form_and_shows_saving", async () => {
    const h = setup();
    let resolve!: (v: { ok: true }) => void;
    mocked(h.actions.saveSettings).mockReturnValue(new Promise((r) => (resolve = r)));
    await userEvent.type(screen.getByLabelText("Focus"), "x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
    expect(screen.getByTestId("settings-fieldset")).toBeDisabled();
    expect(screen.getByLabelText("Focus")).toBeDisabled();
    await act(async () => resolve({ ok: true }));
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
    expect(screen.getByTestId("settings-fieldset")).not.toBeDisabled();
    expect(screen.getByTestId("save-msg")).toHaveTextContent("Saved");
  });

  it("stale_rejection_keeps_draft_and_explains", async () => {
    const h = setup();
    mocked(h.actions.saveSettings).mockResolvedValue({ ok: false, stale: true, message: "stale" });
    await userEvent.type(screen.getByLabelText("Focus"), "Rust");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(STALE_MESSAGE)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("nothing was saved");
    // the store reloads settings (new revision) — the draft survives
    h.update({ ...onSettings, settings: makeSettings({ settingsRevision: 2, alwaysOnTop: false }) });
    expect(screen.getByLabelText("Focus")).toHaveValue("Rust");
    expect(lastDirty(h)).toBe(true);
  });

  it("other_failure_shows_message", async () => {
    const h = setup();
    mocked(h.actions.saveSettings).mockResolvedValue({ ok: false, stale: false, message: "Couldn't encrypt the key." });
    await userEvent.type(screen.getByLabelText("Focus"), "x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't encrypt the key.");
  });

  it("settings_change_adopted_when_clean", () => {
    const h = setup();
    h.update({ ...onSettings, settings: makeSettings({ settingsRevision: 2, hotkey: "Alt+F9" }) });
    expect(screen.getByLabelText("Global shortcut")).toHaveValue("Alt+F9");
  });

  it("dirty_reported_on_edit_and_cleared_after_save", async () => {
    const h = setup();
    await userEvent.type(screen.getByLabelText("Notes"), "n");
    expect(lastDirty(h)).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    // store echoes the saved settings back
    const saved = makeSettings({ settingsRevision: 2 });
    saved.profiles[0]!.notes = "n";
    h.update({ ...onSettings, settings: saved });
    expect(lastDirty(h)).toBe(false);
  });
});

describe("unsaved-changes guard", () => {
  it("back_when_clean_closes", async () => {
    const h = setup();
    await userEvent.click(screen.getByRole("button", { name: "← Back" }));
    expect(h.actions.closeSettings).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId("unsaved-bar")).toBeNull();
  });

  it("back_when_dirty_shows_bar_keep_editing_keeps_draft", async () => {
    const h = setup();
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    await userEvent.click(screen.getByRole("button", { name: "← Back" }));
    expect(screen.getByTestId("unsaved-bar")).toHaveTextContent("You have unsaved changes");
    await waitFor(() => expect(screen.getByRole("button", { name: "Save and go back" })).toHaveFocus());
    expect(h.actions.closeSettings).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(screen.queryByTestId("unsaved-bar")).toBeNull();
    expect(screen.getByLabelText("Focus")).toHaveValue("Go");
    expect(h.actions.closeSettings).not.toHaveBeenCalled();
  });

  it("escape_when_dirty_shows_bar_discard_throws_away", async () => {
    const h = setup();
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByTestId("unsaved-bar")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(h.actions.closeSettings).toHaveBeenCalledTimes(1);
    expect(h.actions.saveSettings).not.toHaveBeenCalled();
    expect(lastDirty(h)).toBe(false);
  });

  it("escape_when_clean_closes", () => {
    const h = setup();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(h.actions.closeSettings).toHaveBeenCalledTimes(1);
  });

  it("save_and_go_back_saves_then_closes", async () => {
    const h = setup();
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    await userEvent.click(screen.getByRole("button", { name: "← Back" }));
    await userEvent.click(screen.getByRole("button", { name: "Save and go back" }));
    await waitFor(() => expect(h.actions.closeSettings).toHaveBeenCalledTimes(1));
    expect(h.actions.saveSettings).toHaveBeenCalledTimes(1);
  });

  it("save_and_go_back_stale_stays_with_draft", async () => {
    const h = setup();
    mocked(h.actions.saveSettings).mockResolvedValue({ ok: false, stale: true, message: "stale" });
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    await userEvent.click(screen.getByRole("button", { name: "← Back" }));
    await userEvent.click(screen.getByRole("button", { name: "Save and go back" }));
    expect(await screen.findByText(STALE_MESSAGE)).toBeInTheDocument();
    expect(h.actions.closeSettings).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Focus")).toHaveValue("Go");
  });

  it("close_requested_opens_bar_and_dismisses_when_resolved", async () => {
    const h = setup();
    await userEvent.type(screen.getByLabelText("Focus"), "Go");
    h.update({ ...onSettings, closeRequested: true });
    expect(screen.getByTestId("unsaved-bar")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(h.actions.dismissCloseRequest).toHaveBeenCalled();
    expect(screen.getByLabelText("Focus")).toHaveValue("Go");
  });

  it("close_requested_when_clean_is_dismissed", () => {
    const h = setup({ closeRequested: true });
    expect(h.actions.dismissCloseRequest).toHaveBeenCalled();
    expect(screen.queryByTestId("unsaved-bar")).toBeNull();
  });
});

describe("session + protection in settings", () => {
  it("session_status_bar_with_stop", async () => {
    const h = setup({ phase: "recording", canStop: true, statusText: "Recording — press Stop & Answer when they finish" });
    const bar = screen.getByTestId("session-bar");
    expect(bar).toHaveTextContent("Recording — press Stop & Answer");
    await userEvent.click(within(bar).getByRole("button", { name: "Stop & Answer" }));
    expect(h.actions.stop).toHaveBeenCalledTimes(1);
  });

  it("session_status_bar_finalizing_has_no_stop", () => {
    setup({ phase: "finalizing", canStop: false, statusText: "Finalizing transcript…" });
    expect(screen.getByTestId("session-bar")).toHaveTextContent("Finalizing transcript…");
    expect(within(screen.getByTestId("session-bar")).queryByRole("button")).toBeNull();
  });

  it("no_session_bar_when_idle", () => {
    setup();
    expect(screen.queryByTestId("session-bar")).toBeNull();
  });

  it("settings_loading_state", () => {
    renderWithView(<SettingsScreen />, { ...onSettings, settings: null });
    expect(screen.getByText("Loading settings…")).toBeInTheDocument();
    expect(screen.getByTestId("protection-badge")).toBeInTheDocument();
  });
});

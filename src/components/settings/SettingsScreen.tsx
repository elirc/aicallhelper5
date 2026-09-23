import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { useApp } from "../../app/view";
import type { AnswerStyle } from "../../generated/AnswerStyle";
import type { SettingsView } from "../../generated/SettingsView";
import { ProtectionBadge } from "../ProtectionBadge";
import { copyText } from "../common/clipboard";
import { ACTIVE_PHASES, STYLES } from "../common/labels";
import { HOTKEY_MAX, buildPatch, draftFromSettings, hotkeyProblem, isDirty, type SecretDraft, type SettingsDraft } from "./draft";
import { KeysSection } from "./KeysSection";
import { ProfileEditor } from "./ProfileEditor";

export const STALE_MESSAGE = "Settings changed elsewhere — nothing was saved. Review and save again.";
export const SINGLE_TURN_NOTE = "Answers are single-turn: previous questions are never sent as context.";

export function hotkeyStatusText(s: Pick<SettingsView, "hotkey" | "hotkeyStatus" | "hotkeyMessage">): string {
  let base: string;
  switch (s.hotkeyStatus) {
    case "registered":
      base = `Registered — ${s.hotkey} toggles recording from any app.`;
      break;
    case "disabled":
      base = "Disabled — no global shortcut is set.";
      break;
    case "invalid":
      base = "Not registered — the saved shortcut is not valid.";
      break;
    default:
      base = "Not registered — Windows would not give the app this shortcut (another app may be using it).";
  }
  return s.hotkeyMessage ? `${base} ${s.hotkeyMessage}` : base;
}

type SaveMsg = { kind: "ok" | "error"; text: string } | null;
type GuardReason = "back" | "close";

export function SettingsScreen() {
  const { view } = useApp();
  if (!view.settings) {
    return (
      <div className="layout layout--settings" data-testid="settings-screen">
        <p className="empty-hint">Loading settings…</p>
        <ProtectionBadge />
      </div>
    );
  }
  return <SettingsForm settings={view.settings} />;
}

function SettingsForm({ settings }: { settings: SettingsView }) {
  const { view, actions } = useApp();
  const [draft, setDraft] = useState<SettingsDraft>(() => draftFromSettings(settings));
  const [editingId, setEditingId] = useState(settings.activeProfileId);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<SaveMsg>(null);
  const [guard, setGuard] = useState<GuardReason | null>(null);
  const [diagMsg, setDiagMsg] = useState<string | null>(null);
  const prevSettings = useRef(settings);
  const barRef = useRef<HTMLDivElement>(null);

  // Adopt fresh settings unless the user has edits that differ from both the
  // old and the new settings (stale rejection: keep the draft).
  useEffect(() => {
    const prev = prevSettings.current;
    if (prev === settings) return;
    prevSettings.current = settings;
    setDraft((d) => {
      const plain = { ...d, secrets: {} };
      if (!isDirty(prev, plain) || !isDirty(settings, plain)) {
        return { ...draftFromSettings(settings), secrets: d.secrets };
      }
      return d;
    });
  }, [settings]);

  const dirty = isDirty(settings, draft);
  const { setSettingsDirty, dismissCloseRequest, closeSettings } = actions;

  useEffect(() => {
    setSettingsDirty(dirty);
  }, [dirty, setSettingsDirty]);

  useEffect(() => () => setSettingsDirty(false), [setSettingsDirty]);

  // The core asked to close while we were dirty → show the unsaved bar.
  useEffect(() => {
    if (!view.closeRequested) return;
    if (dirty) setGuard("close");
    else dismissCloseRequest();
  }, [view.closeRequested, dirty, dismissCloseRequest]);

  // Move focus into the unsaved bar when it opens (keyboard users land on it).
  useEffect(() => {
    if (!guard) return;
    const id = setTimeout(() => barRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus(), 0);
    return () => clearTimeout(id);
  }, [guard]);

  const resolveGuard = useCallback(() => {
    if (guard === "close" || view.closeRequested) dismissCloseRequest();
    setGuard(null);
  }, [guard, view.closeRequested, dismissCloseRequest]);

  const requestBack = useCallback(() => {
    if (saving) return;
    if (dirty) setGuard((g) => g ?? "back");
    else closeSettings();
  }, [saving, dirty, closeSettings]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (guard) resolveGuard();
      else requestBack();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [guard, resolveGuard, requestBack]);

  const hotkeyHint = hotkeyProblem(draft.hotkey);
  const emptyName = draft.profiles.some((p) => !p.name.trim());
  const writesBlocked = !!settings.loadIssue?.writesBlocked;
  const blocker = writesBlocked
    ? "Settings can't be saved right now."
    : emptyName
      ? "Every profile needs a name."
      : hotkeyHint
        ? "Fix the global shortcut first."
        : null;

  async function save(): Promise<boolean> {
    if (blocker || saving) return false;
    const patch = buildPatch(settings, draft);
    if (Object.keys(patch).length === 0) {
      setSaveMsg({ kind: "ok", text: "Nothing to save" });
      return true;
    }
    setSaving(true);
    setSaveMsg(null);
    try {
      const res = await actions.saveSettings(patch);
      if (res.ok) {
        setDraft((d) => ({ ...d, secrets: {} }));
        setSaveMsg({ kind: "ok", text: "Saved" });
        return true;
      }
      setSaveMsg({ kind: "error", text: res.stale ? STALE_MESSAGE : res.message });
      return false;
    } catch {
      setSaveMsg({ kind: "error", text: "Saving failed. Try again." });
      return false;
    } finally {
      setSaving(false);
    }
  }

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    void save();
  }

  async function saveAndGoBack() {
    const ok = await save();
    if (!ok) {
      setGuard(null);
      return;
    }
    resolveGuard();
    closeSettings();
  }

  function discard() {
    setDraft(draftFromSettings(settings));
    setEditingId(settings.activeProfileId);
    setSaveMsg(null);
    setSettingsDirty(false);
    resolveGuard();
    closeSettings();
  }

  async function onCopyDiagnostics() {
    setDiagMsg(null);
    const text = await actions.copyDiagnostics();
    if (text === null) {
      setDiagMsg("Couldn't collect diagnostics.");
      return;
    }
    await copyText(text);
    setDiagMsg("Diagnostics copied");
  }

  function setSecret(keyId: string, next: SecretDraft) {
    setSaveMsg(null);
    setDraft((d) => ({ ...d, secrets: { ...d.secrets, [keyId]: next } }));
  }

  function patchDraft(p: Partial<SettingsDraft>) {
    setSaveMsg(null);
    setDraft((d) => ({ ...d, ...p }));
  }

  const sessionActive = ACTIVE_PHASES.has(view.phase);
  const issue = settings.loadIssue;
  const version = `v${settings.build.version}`;
  const rev = `${settings.build.gitRevision}${settings.build.dirty ? "-dirty" : ""}`;

  return (
    <div className="layout layout--settings" data-testid="settings-screen">
      {sessionActive && (
        <div className="session-bar" data-testid="session-bar">
          <p className="session-bar__text" aria-live="polite">
            {view.statusText}
          </p>
          {view.canStop && (
            <button type="button" className="btn btn--small btn--stop" onClick={() => actions.stop()}>
              Stop & Answer
            </button>
          )}
        </div>
      )}
      <header className="settings-header">
        <button type="button" className="btn btn--ghost btn--small" onClick={requestBack} disabled={saving}>
          ← Back
        </button>
        <h1 className="settings-header__title">Settings</h1>
        <span className="version-chip" title={`Build ${settings.build.buildTime}`} data-testid="version-chip">
          {version} · {rev}
        </span>
      </header>
      <ProtectionBadge />

      {guard && (
        <div ref={barRef} className="unsaved-bar" role="alertdialog" aria-labelledby="unsaved-text" data-testid="unsaved-bar">
          <p id="unsaved-text" className="unsaved-bar__text">
            You have unsaved changes
          </p>
          <div className="unsaved-bar__actions">
            <button
              type="button"
              className="btn btn--small btn--primary"
              disabled={saving || !!blocker}
              onClick={() => void saveAndGoBack()}
            >
              Save and go back
            </button>
            <button type="button" className="btn btn--small btn--danger" disabled={saving} onClick={discard}>
              Discard
            </button>
            <button type="button" className="btn btn--small" disabled={saving} onClick={resolveGuard}>
              Keep editing
            </button>
          </div>
        </div>
      )}

      {issue && (
        <div className="banner banner--warn" data-testid="load-issue">
          <p>{issue.message}</p>
          {issue.backupPath && <p className="banner__detail">Backup: {issue.backupPath}</p>}
          {issue.writesBlocked && (
            <p role="alert" className="banner__alert">
              Settings can't be saved: the original file couldn't be backed up, so the app won't overwrite it.
            </p>
          )}
        </div>
      )}

      <form className="settings-form" onSubmit={onSubmit} noValidate>
        <fieldset className="plain-fieldset" disabled={saving} data-testid="settings-fieldset">
          <legend className="visually-hidden">Settings</legend>
          <KeysSection keys={settings.keys} secrets={draft.secrets} onChange={setSecret} onGetKey={(url) => actions.openExternal(url)} />

          <ProfileEditor
            profiles={draft.profiles}
            activeProfileId={draft.activeProfileId}
            editingId={editingId}
            onEditingChange={setEditingId}
            onProfilesChange={(profiles, activeProfileId) =>
              patchDraft(activeProfileId === undefined ? { profiles } : { profiles, activeProfileId })
            }
          />

          <section className="settings-section" aria-labelledby="answers-heading">
            <h2 id="answers-heading" className="settings-section__title">
              Answers
            </h2>
            <label className="field">
              <span className="field__label">Answer provider</span>
              <select className="select" value={draft.llmProvider} onChange={(e) => patchDraft({ llmProvider: e.target.value })}>
                {settings.providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.displayName}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span className="field__label">Answer style</span>
              <select className="select" value={draft.answerStyle} onChange={(e) => patchDraft({ answerStyle: e.target.value as AnswerStyle })}>
                {STYLES.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.label}
                  </option>
                ))}
              </select>
            </label>
            <p className="settings-note">{SINGLE_TURN_NOTE}</p>
          </section>

          <section className="settings-section" aria-labelledby="window-heading">
            <h2 id="window-heading" className="settings-section__title">
              Shortcut and window
            </h2>
            <label className="field">
              <span className="field__label">Global shortcut</span>
              <input
                className="input"
                type="text"
                value={draft.hotkey}
                maxLength={HOTKEY_MAX}
                spellCheck={false}
                placeholder="e.g. Ctrl+Shift+Space (empty = off)"
                aria-invalid={hotkeyHint ? true : undefined}
                aria-describedby="hotkey-status hotkey-hint"
                onChange={(e) => patchDraft({ hotkey: e.target.value })}
              />
            </label>
            <p id="hotkey-hint" className={`field__hint${hotkeyHint ? " field__hint--error" : ""}`} data-testid="hotkey-hint">
              {hotkeyHint ?? "Any of Ctrl, Alt, Shift or Win plus one key. Leave empty to turn it off."}
            </p>
            <p id="hotkey-status" className={`field__hint hotkey-status hotkey-status--${settings.hotkeyStatus}`} data-testid="hotkey-status">
              {hotkeyStatusText(settings)}
            </p>
            <label className="check">
              <input type="checkbox" checked={draft.alwaysOnTop} onChange={(e) => patchDraft({ alwaysOnTop: e.target.checked })} />
              <span>Keep window on top</span>
            </label>
          </section>

          <section className="settings-section" aria-labelledby="about-heading">
            <h2 id="about-heading" className="settings-section__title">
              About
            </h2>
            <div className="about-row">
              <button type="button" className="btn btn--small" onClick={() => void onCopyDiagnostics()}>
                Copy diagnostics
              </button>
              <span className="settings-note" aria-live="polite">
                {diagMsg ?? ""}
              </span>
            </div>
          </section>

          <div className="save-row">
            <button type="submit" className="btn btn--primary" disabled={saving || !!blocker}>
              {saving ? "Saving…" : "Save"}
            </button>
            <span
              className={`save-msg${saveMsg?.kind === "error" ? " save-msg--error" : ""}`}
              role={saveMsg?.kind === "error" ? "alert" : "status"}
              data-testid="save-msg"
            >
              {saveMsg?.text ?? (blocker && dirty ? blocker : "")}
            </span>
          </div>
        </fieldset>
      </form>
    </div>
  );
}

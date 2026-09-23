// Shared test helpers for component tests (PINNED; both frontend agents use it).
import type { SettingsView } from "../generated/SettingsView";
import type { AppActions, AppView } from "./view";

export function makeSettings(over: Partial<SettingsView> = {}): SettingsView {
  return {
    settingsRevision: 1,
    profiles: [
      {
        id: "default",
        name: "Default",
        callType: "behavioral",
        focus: "",
        resume: "",
        jobDescription: "",
        notes: "",
      },
    ],
    activeProfileId: "default",
    llmProvider: "anthropic",
    answerStyle: "balanced",
    hotkey: "Ctrl+Shift+Space",
    alwaysOnTop: true,
    layoutMode: "full",
    prompterFontPx: 18,
    answerFontPx: 14,
    keys: [
      { id: "deepgram", label: "Deepgram", hasKey: true, storage: "encrypted", getKeyUrl: "https://console.deepgram.com/" },
      { id: "anthropic", label: "Anthropic", hasKey: true, storage: "encrypted", getKeyUrl: "https://console.anthropic.com/settings/keys" },
      { id: "groq", label: "Groq", hasKey: false, storage: "unset", getKeyUrl: "https://console.groq.com/keys" },
    ],
    providers: [
      { id: "anthropic", displayName: "Claude Haiku 4.5 (recommended)", keyId: "anthropic", model: "claude-haiku-4-5" },
      { id: "groq", displayName: "Groq GPT-OSS 120B (fastest)", keyId: "groq", model: "openai/gpt-oss-120b" },
    ],
    hotkeyRegistered: true,
    hotkeyStatus: "registered",
    hotkeyMessage: null,
    loadIssue: null,
    settingsPath: "C:\\Users\\me\\AppData\\Roaming\\AICallAssistant\\settings.json",
    build: { version: "4.0.0", gitRevision: "abc1234", dirty: false, buildTime: "2026-01-01T00:00:00Z" },
    ...over,
  };
}

export function makeView(over: Partial<AppView> = {}): AppView {
  const settings = over.settings === undefined ? makeSettings() : over.settings;
  return {
    core: "ready",
    coreError: null,
    protection: "protected",
    phase: "idle",
    statusText: "Ready — press Record while the other person is speaking",
    recording: null,
    liveTranscript: "",
    entry: null,
    history: { index: 0, count: 0, canPrev: false, canNext: false },
    sessionError: null,
    notice: null,
    settings,
    activeProfile: settings?.profiles[0] ?? null,
    providerName: "Claude Haiku 4.5 (recommended)",
    answerStyle: "balanced",
    layout: "full",
    answerFontPx: 14,
    prompterFontPx: 18,
    hotkeyHint: "Ctrl+Shift+Space toggles recording from any app",
    screen: "main",
    needsSetup: false,
    canRecord: true,
    canStop: false,
    canAsk: true,
    canRegenerate: false,
    settingsDirty: false,
    closeRequested: false,
    ...over,
  };
}

/** Every action is a vitest mock (vi must be in scope — tests import it). */
export function makeActions(mockFn: () => (...args: never[]) => unknown): AppActions {
  const m = () => mockFn() as never;
  return {
    record: m(), stop: m(), toggleRecord: m(), ask: m(), cancel: m(), regenerate: m(),
    historyPrev: m(), historyNext: m(), clearHistory: m(), setStyle: m(), setCallType: m(),
    setActiveProfile: m(), setLayout: m(), bumpAnswerFont: m(), bumpPrompterFont: m(),
    dock: m(), openSettings: m(), closeSettings: m(), saveSettings: m(), setSettingsDirty: m(),
    dismissCloseRequest: m(), openExternal: m(), copyDiagnostics: m(), dismissNotice: m(),
    reportFirstPaint: m(),
  };
}

//! Types that cross the core <-> page boundary. All serialize camelCase and are
//! exported to `src/generated/` with ts-rs. `u64` fields that hold epoch-ms,
//! revisions or seq numbers are typed `number` in TS (safe below 2^53).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ───────────────────────────── errors ─────────────────────────────

/// Closed error set (spec §10). The UI keys BEHAVIOR off the code; `message`
/// is actionable, user-facing copy (never a stack trace, never a secret).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    NoSttKey,
    NoLlmKey,
    SttConnect,
    SttError,
    SttTimeout,
    NoSpeech,
    LlmAuth,
    LlmHttp,
    LlmRateLimit,
    LlmFirstTokenTimeout,
    LlmTimeout,
    Aborted,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[error("{code:?}: {message}")]
#[ts(export)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
    pub fn aborted() -> Self {
        Self::new(ErrorCode::Aborted, "Cancelled.")
    }
}

/// Canonical user-facing copy. Crates should use these instead of inventing
/// variants of the same sentence.
pub mod copy {
    pub const NO_SPEECH: &str =
        "No speech detected in the recording. Make sure call audio is playing.";
    pub const DEVICE_OPEN: &str = "Could not open the system audio device. Check that a default output device exists, then try again.";
    pub const DEVICE_LOST: &str = "The system audio device was disconnected during recording. Answering with what was captured.";
    pub const DEVICE_CHANGED: &str =
        "The default output device changed — now capturing the new device.";
    pub const STT_CONNECT: &str =
        "Could not connect to Deepgram. Check the API key and your network.";
    pub const NO_STT_KEY: &str = "Add your Deepgram API key in Settings to record.";
    pub const NO_LLM_KEY: &str = "Add an API key for the selected answer provider in Settings.";
    pub const FIRST_TOKEN_TIMEOUT: &str = "The answer didn't start streaming within 10 seconds. Try again, or switch the answer provider in Settings.";
    pub const TOTAL_TIMEOUT: &str = "The answer took longer than 60 seconds and was stopped.";
    pub const STT_TIMEOUT: &str =
        "Deepgram did not finish the transcript within 5 seconds. Try again.";
    pub const CORE_FAILED: &str = "The app core failed to start. Restart the app; if it keeps happening, copy diagnostics from Settings.";
    pub const CORE_STARTING: &str = "The app is still starting. Try again in a moment.";
    pub const STOP_NOT_TAKEN: &str = "That recording is no longer active.";
    pub const EMPTY_QUESTION: &str = "Type a question first.";
}

// ───────────────────────────── ids & enums ─────────────────────────────

/// "s1", "s2", … — unique per process run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CallType {
    #[default]
    Behavioral,
    Technical,
    SystemDesign,
    Recruiter,
    Sales,
    Meeting,
}

impl CallType {
    pub const ALL: [CallType; 6] = [
        CallType::Behavioral,
        CallType::Technical,
        CallType::SystemDesign,
        CallType::Recruiter,
        CallType::Sales,
        CallType::Meeting,
    ];
    pub fn id(self) -> &'static str {
        match self {
            CallType::Behavioral => "behavioral",
            CallType::Technical => "technical",
            CallType::SystemDesign => "system_design",
            CallType::Recruiter => "recruiter",
            CallType::Sales => "sales",
            CallType::Meeting => "meeting",
        }
    }
    /// Unknown ids fall back to `Behavioral` (spec §8).
    pub fn from_id_lossy(id: &str) -> CallType {
        CallType::ALL
            .into_iter()
            .find(|c| c.id() == id)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AnswerStyle {
    Brief,
    #[default]
    Balanced,
    Detailed,
}

impl AnswerStyle {
    /// Unknown ids fall back to `Balanced` (spec §8).
    pub fn from_id_lossy(id: &str) -> AnswerStyle {
        match id {
            "brief" => AnswerStyle::Brief,
            "detailed" => AnswerStyle::Detailed,
            _ => AnswerStyle::Balanced,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LayoutMode {
    #[default]
    Full,
    Prompter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Finish {
    Complete,
    Truncated,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Phase {
    Idle,
    Starting,
    Recording,
    Finalizing,
    Answering,
    /// Only in `get_status` when the core was too busy to answer within 2 s.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CoreState {
    Starting,
    Ready,
    Failed,
}

/// Content-protection verdict. Starts `Unknown`; only Windows' read-back of
/// the display affinity can make it `Protected`. The page can never set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Protection {
    Protected,
    Unprotected,
    Unknown,
}

// ───────────────────────────── status snapshot ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordingInfo {
    /// Epoch milliseconds at which the core auto-stops (the single deadline
    /// both the core cap and the UI countdown use).
    #[ts(type = "number")]
    pub deadline_ms: u64,
    pub cap_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionStatus {
    pub id: Option<SessionId>,
    pub phase: Phase,
    pub recording: Option<RecordingInfo>,
}

impl SessionStatus {
    pub fn idle() -> Self {
        Self {
            id: None,
            phase: Phase::Idle,
            recording: None,
        }
    }
}

/// `get_status()` value. `revision` increases whenever core state, protection
/// or session changes; the page adopts a snapshot/event only if its revision
/// is >= the last one it saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatusSnapshot {
    #[ts(type = "number")]
    pub revision: u64,
    pub core: CoreState,
    pub core_error: Option<AppError>,
    pub protection: Protection,
    pub session: SessionStatus,
}

// ───────────────────────────── metrics ─────────────────────────────

/// Integer milliseconds. `first_token_ms` and `total_ms` are measured from Stop
/// acceptance (or ask acceptance) and INCLUDE the audio drain. `audio_drain_ms`
/// is from when the drain actually starts (after the STT connect if Stop landed
/// during connect) to drain complete; `stt_finalize_ms` is drain complete ->
/// final transcript. For `ask`, both are 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Metrics {
    pub audio_drain_ms: u32,
    pub stt_finalize_ms: u32,
    pub first_token_ms: u32,
    pub total_ms: u32,
}

// ───────────────────────────── events (core -> page) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DeviceNoticeKind {
    /// Device disappeared mid-recording; the session auto-stops and answers.
    Lost,
    /// Default output device switched; capture followed it.
    Changed,
}

/// Every event the core sends. Serialized internally tagged:
/// `{ "type": "stt:partial", "sessionId": "s3", "text": "...", "isFinal": false }`.
/// Session events carry `sessionId`; the page drops events for a session id
/// that is not its current one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum CoreEvent {
    #[serde(rename = "session:recording", rename_all = "camelCase")]
    SessionRecording {
        session_id: SessionId,
        #[ts(type = "number")]
        deadline_ms: u64,
        cap_ms: u32,
    },
    /// `text` is the FULL transcript so far (finalized segments + interim).
    #[serde(rename = "stt:partial", rename_all = "camelCase")]
    SttPartial {
        session_id: SessionId,
        text: String,
        is_final: bool,
    },
    /// RMS 0..1. Coalesced latest-wins per session; stops after Stop.
    #[serde(rename = "audio:level", rename_all = "camelCase")]
    AudioLevel { session_id: SessionId, rms: f32 },
    #[serde(rename = "audio:device", rename_all = "camelCase")]
    AudioDevice {
        session_id: SessionId,
        kind: DeviceNoticeKind,
        message: String,
    },
    /// The 120 s cap tripped; the session is finalizing/answering now.
    #[serde(rename = "session:autostopped", rename_all = "camelCase")]
    SessionAutostopped { session_id: SessionId },
    #[serde(rename = "llm:delta", rename_all = "camelCase")]
    LlmDelta {
        session_id: SessionId,
        delta: String,
    },
    #[serde(rename = "llm:done", rename_all = "camelCase")]
    LlmDone {
        session_id: SessionId,
        transcript: String,
        answer: String,
        finish: Finish,
        call_type: CallType,
        metrics: Metrics,
    },
    #[serde(rename = "session:error", rename_all = "camelCase")]
    SessionError {
        session_id: SessionId,
        error: AppError,
    },

    // ── window-scoped ──
    #[serde(rename = "hotkey:toggle")]
    HotkeyToggle {},
    #[serde(rename = "protection:ok")]
    ProtectionOk {
        #[ts(type = "number")]
        revision: u64,
        protection: Protection,
    },
    #[serde(rename = "protection:failed")]
    ProtectionFailed {
        #[ts(type = "number")]
        revision: u64,
        protection: Protection,
    },
    #[serde(rename = "core:ready")]
    CoreReady {
        #[ts(type = "number")]
        revision: u64,
    },
    #[serde(rename = "core:failed")]
    CoreFailed {
        #[ts(type = "number")]
        revision: u64,
        error: AppError,
    },
    #[serde(rename = "window:close-requested")]
    WindowCloseRequested {},
    /// The settings file on disk changed state (e.g. hotkey registration
    /// result arrived late). The page should re-fetch `get_settings`.
    #[serde(rename = "settings:changed")]
    SettingsChanged {},
}

impl CoreEvent {
    pub fn session_id(&self) -> Option<&SessionId> {
        use CoreEvent::*;
        match self {
            SessionRecording { session_id, .. }
            | SttPartial { session_id, .. }
            | AudioLevel { session_id, .. }
            | AudioDevice { session_id, .. }
            | SessionAutostopped { session_id }
            | LlmDelta { session_id, .. }
            | LlmDone { session_id, .. }
            | SessionError { session_id, .. } => Some(session_id),
            _ => None,
        }
    }

    /// Terminal and protection/core events must never be dropped, even under
    /// backpressure.
    pub fn is_must_deliver(&self) -> bool {
        use CoreEvent::*;
        matches!(
            self,
            LlmDone { .. }
                | SessionError { .. }
                | ProtectionOk { .. }
                | ProtectionFailed { .. }
                | CoreReady { .. }
                | CoreFailed { .. }
                | WindowCloseRequested {}
                | SessionAutostopped { .. }
                | AudioDevice { .. }
        )
    }

    /// Audio levels coalesce latest-wins.
    pub fn is_coalescable(&self) -> bool {
        matches!(self, CoreEvent::AudioLevel { .. })
    }
}

/// What actually goes over the Tauri Channel: the event plus a monotonic `seq`
/// assigned by the shell's event pump (per process, strictly increasing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EventEnvelope {
    #[ts(type = "number")]
    pub seq: u64,
    #[serde(flatten)]
    pub event: CoreEvent,
}

// ───────────────────────────── settings ─────────────────────────────

pub const MAX_PROFILES: usize = 20;
pub const PROFILE_NAME_MAX: usize = 60;
pub const FOCUS_MAX: usize = 2_000;
pub const PROFILE_TEXT_MAX: usize = 200_000;
pub const HOTKEY_MAX: usize = 100;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Shift+Space";
pub const PROMPTER_FONT_MIN: u8 = 14;
pub const PROMPTER_FONT_MAX: u8 = 28;
pub const PROMPTER_FONT_DEFAULT: u8 = 18;
pub const ANSWER_FONT_MIN: u8 = 12;
pub const ANSWER_FONT_MAX: u8 = 22;
pub const ANSWER_FONT_DEFAULT: u8 = 14;
pub const FONT_STEP: u8 = 2;

/// Profile ids match `^[A-Za-z0-9_-]{1,64}$`. Profiles are stored in plain text.
/// `Debug` omits the free-text fields (they must never reach logs/panics).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub call_type: CallType,
    pub focus: String,
    pub resume: String,
    pub job_description: String,
    pub notes: String,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile")
            .field("id", &self.id)
            .field("call_type", &self.call_type)
            .field("name_len", &self.name.len())
            .field("focus_len", &self.focus.len())
            .field("resume_len", &self.resume.len())
            .field("job_description_len", &self.job_description.len())
            .field("notes_len", &self.notes.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderInfo {
    /// e.g. "anthropic", "groq".
    pub id: String,
    /// e.g. "Claude Haiku 4.5 (recommended)".
    pub display_name: String,
    /// The secret this provider needs, e.g. "anthropic".
    pub key_id: String,
    /// The model id actually sent (single constant per provider).
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KeyStorage {
    /// No key stored.
    Unset,
    /// Stored DPAPI-encrypted (current user scope).
    Encrypted,
    /// A value exists but could not be decrypted; it reads as unset.
    Unreadable,
}

/// Write-only secrets: the view exposes only whether a key exists and how it
/// is stored — key material never enters a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyStatus {
    /// "deepgram" | "anthropic" | "groq" | …
    pub id: String,
    /// "Deepgram", "Anthropic", "Groq".
    pub label: String,
    pub has_key: bool,
    pub storage: KeyStorage,
    /// Where to get a key (https).
    pub get_key_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HotkeyStatus {
    Registered,
    /// Empty hotkey string.
    Disabled,
    /// Could not be parsed as Ctrl/Alt/Shift/Win + key.
    Invalid,
    /// Valid, but another app owns it (or registration failed).
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SettingsLoadStatus {
    /// File existed and loaded (possibly with per-field fallbacks).
    Ok,
    /// No file — first run.
    FirstRun,
    /// File could not be read (IO error).
    Unreadable,
    /// File was not valid JSON / not an object.
    Corrupt,
}

/// Surfaced in the UI when the settings file was unreadable or corrupt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsLoadIssue {
    pub status: SettingsLoadStatus,
    /// Where the original was backed up, once it has been.
    pub backup_path: Option<String>,
    /// If true, the backup failed and the app REFUSES to write settings.
    pub writes_blocked: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BuildInfo {
    pub version: String,
    pub git_revision: String,
    pub dirty: bool,
    pub build_time: String,
}

/// `get_settings()` / `set_settings()` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsView {
    #[ts(type = "number")]
    pub settings_revision: u64,
    pub profiles: Vec<Profile>,
    pub active_profile_id: String,
    pub llm_provider: String,
    pub answer_style: AnswerStyle,
    pub hotkey: String,
    pub always_on_top: bool,
    pub layout_mode: LayoutMode,
    pub prompter_font_px: u8,
    pub answer_font_px: u8,
    /// One entry per secret the app knows (deepgram + every provider key).
    pub keys: Vec<KeyStatus>,
    /// Provider registry: `{id, displayName, keyId, model}`.
    pub providers: Vec<ProviderInfo>,
    pub hotkey_registered: bool,
    pub hotkey_status: HotkeyStatus,
    /// Human-readable reason when the hotkey is not registered.
    pub hotkey_message: Option<String>,
    pub load_issue: Option<SettingsLoadIssue>,
    pub settings_path: String,
    pub build: BuildInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum SecretAction {
    /// Store a new key (DPAPI). Fails closed: on encryption failure the save
    /// is refused and the previous key is untouched.
    Set { value: String },
    /// Explicitly remove the stored key.
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SecretChange {
    pub key_id: String,
    #[serde(flatten)]
    pub action: SecretAction,
}

/// `set_settings(patch)`. Every field optional except `baseRevision`; a patch
/// whose `baseRevision` != current `settingsRevision` is rejected and nothing
/// is applied. `profiles`, when present, replaces the whole list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsPatch {
    #[ts(type = "number")]
    pub base_revision: u64,
    #[ts(optional)]
    pub profiles: Option<Vec<Profile>>,
    #[ts(optional)]
    pub active_profile_id: Option<String>,
    #[ts(optional)]
    pub llm_provider: Option<String>,
    #[ts(optional)]
    pub answer_style: Option<AnswerStyle>,
    #[ts(optional)]
    pub hotkey: Option<String>,
    #[ts(optional)]
    pub always_on_top: Option<bool>,
    #[ts(optional)]
    pub layout_mode: Option<LayoutMode>,
    #[ts(optional)]
    pub prompter_font_px: Option<u8>,
    #[ts(optional)]
    pub answer_font_px: Option<u8>,
    #[ts(optional)]
    pub secrets: Option<Vec<SecretChange>>,
}

/// Window geometry in physical pixels (virtual-screen coordinates, may be
/// negative). Not part of the view; persisted per layout by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

// ───────────────────────────── command result envelope ─────────────────────────────

/// Every command returns `{ok:true, value} | {ok:false, error}`. Nothing panics
/// or throws across the boundary. (TS side: `CmdResult<T>` in
/// `src/ipc/types.ts`, hand-written because ts-rs can't express this shape.)
#[derive(Debug, Clone, PartialEq)]
pub enum CmdResult<T> {
    Ok(T),
    Err(AppError),
}

impl<T> From<Result<T, AppError>> for CmdResult<T> {
    fn from(r: Result<T, AppError>) -> Self {
        match r {
            Ok(v) => CmdResult::Ok(v),
            Err(e) => CmdResult::Err(e),
        }
    }
}

impl<T: Serialize> Serialize for CmdResult<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("CmdResult", 2)?;
        match self {
            CmdResult::Ok(v) => {
                st.serialize_field("ok", &true)?;
                st.serialize_field("value", v)?;
            }
            CmdResult::Err(e) => {
                st.serialize_field("ok", &false)?;
                st.serialize_field("error", e)?;
            }
        }
        st.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_envelope_wire_shape() {
        let env = EventEnvelope {
            seq: 7,
            event: CoreEvent::SttPartial {
                session_id: SessionId("s3".into()),
                text: "hi".into(),
                is_final: false,
            },
        };
        assert_eq!(
            serde_json::to_value(&env).unwrap(),
            json!({"seq":7,"type":"stt:partial","sessionId":"s3","text":"hi","isFinal":false})
        );
        let done = CoreEvent::LlmDone {
            session_id: SessionId("s1".into()),
            transcript: "q".into(),
            answer: "a".into(),
            finish: Finish::Complete,
            call_type: CallType::SystemDesign,
            metrics: Metrics {
                audio_drain_ms: 1,
                stt_finalize_ms: 2,
                first_token_ms: 3,
                total_ms: 4,
            },
        };
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            json!({"type":"llm:done","sessionId":"s1","transcript":"q","answer":"a","finish":"complete","callType":"system_design","metrics":{"audioDrainMs":1,"sttFinalizeMs":2,"firstTokenMs":3,"totalMs":4}})
        );
        assert_eq!(
            serde_json::to_value(CoreEvent::HotkeyToggle {}).unwrap(),
            json!({"type":"hotkey:toggle"})
        );
    }

    #[test]
    fn cmd_result_wire_shape() {
        let ok: CmdResult<Option<u8>> = CmdResult::Ok(None);
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({"ok":true,"value":null})
        );
        let err: CmdResult<()> = CmdResult::Err(AppError::new(ErrorCode::NoSpeech, "x"));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"ok":false,"error":{"code":"no_speech","message":"x"}})
        );
    }

    #[test]
    fn secret_change_wire_shape() {
        let c: SecretChange =
            serde_json::from_value(json!({"keyId":"groq","action":"set","value":"k"})).unwrap();
        assert_eq!(c.action, SecretAction::Set { value: "k".into() });
        let r: SecretChange =
            serde_json::from_value(json!({"keyId":"groq","action":"remove"})).unwrap();
        assert_eq!(r.action, SecretAction::Remove);
    }

    #[test]
    fn lossy_fallbacks() {
        assert_eq!(CallType::from_id_lossy("nope"), CallType::Behavioral);
        assert_eq!(
            CallType::from_id_lossy("system_design"),
            CallType::SystemDesign
        );
        assert_eq!(AnswerStyle::from_id_lossy("??"), AnswerStyle::Balanced);
    }
}

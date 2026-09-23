//! Ports: every external dependency of the session machine sits behind one of
//! these traits. Production implementations live in their own crates
//! (`callcore-audio`, `callcore-stt`, `callcore-llm`, `callcore-settings`) or
//! in the Tauri shell; tests use fakes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::types::{AnswerStyle, CallType, CoreEvent, Finish, Profile};
use crate::Secret;

// ───────────────────────────── audio ─────────────────────────────

/// One 2048-sample (128 ms) frame of 16 kHz mono i16 PCM, plus its RMS (0..1).
/// The FINAL frame delivered by a drain may be shorter than 2048 samples.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub rms: f32,
}

impl AudioFrame {
    /// Little-endian bytes for a linear16 WebSocket binary message.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.samples.len() * 2);
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

/// What the capture side pushes into the session.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioMsg {
    Frame(AudioFrame),
    /// The capture device disappeared mid-recording (unplug, driver reset).
    /// No further frames will arrive for this capture.
    DeviceLost {
        message: String,
    },
    /// The default render device changed and capture switched to it.
    DeviceChanged {
        message: String,
    },
}

/// Unbounded on purpose: the audio thread must never block; total volume is
/// bounded by the 120 s record cap (~940 frames).
pub type FrameSink = mpsc::UnboundedSender<AudioMsg>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    /// Could not open / start the loopback device. Message is user copy.
    #[error("{0}")]
    DeviceOpen(String),
    #[error("audio worker is gone")]
    WorkerGone,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainReport {
    /// Number of frames (including the final partial one) pushed during the drain.
    pub frames_flushed: usize,
    /// True when the 2 s bound was hit before all audio was delivered.
    pub timed_out: bool,
}

/// Loopback capture of the default render device.
///
/// Contract: all blocking device calls run serialized on ONE dedicated audio
/// worker thread, so these async methods only send a request to that thread
/// and await its reply — the tokio runtime never blocks.
#[async_trait]
pub trait AudioSource: Send + Sync {
    /// Open the device and start pushing frames into `sink`. Returns once
    /// capture is ACTUALLY running (the session arms the record cap then).
    async fn start(&self, sink: FrameSink) -> Result<(), AudioError>;

    /// Stop capture and DISCARD anything buffered (cancel / supersede path).
    /// Clears the sink so a late callback can never post into a later session.
    async fn stop_discard(&self);

    /// Stop device input and deliver EVERY pre-stop sample — including the
    /// final partial frame — into the sink, then clear the sink. Returns when
    /// all of it has been pushed (the "capture cutoff"), bounded by `timeout`.
    async fn stop_and_drain(&self, timeout: Duration) -> Result<DrainReport, AudioError>;
}

// ───────────────────────────── speech-to-text ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttFailureKind {
    /// 401 / close code 1008 — bad or revoked key.
    BadKey,
    /// Could not establish the socket (DNS, TLS, refused, handshake).
    Connect,
    /// Server error / unexpected close mid-stream.
    Server,
    /// Local send failed because the socket is gone.
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SttFailure {
    pub kind: SttFailureKind,
    /// User-facing copy, e.g. "Deepgram closed the connection (code 1008 …)".
    pub message: String,
}

/// Events from the STT reader task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEvent {
    /// Accumulated FULL transcript so far (finalized segments + current interim).
    Transcript { text: String, is_final: bool },
    /// The server flushed everything after CloseStream and closed cleanly.
    /// `transcript` is the final accumulated text (may be empty).
    Flushed { transcript: String },
    /// The stream failed. After `Flushed` has been delivered, no `Failed`
    /// follows (a late close after the final transcript is not an error).
    Failed(SttFailure),
}

pub struct SttConnection {
    pub sender: Box<dyn SttSender>,
    pub events: mpsc::Receiver<SttEvent>,
}

#[async_trait]
pub trait SttConnector: Send + Sync {
    /// Open a streaming recognition socket. The caller applies the 5 s
    /// connect timeout. KeepAlive is the implementation's job.
    async fn connect(&self, key: &Secret) -> Result<SttConnection, SttFailure>;
}

#[async_trait]
pub trait SttSender: Send {
    /// Send one frame as a binary message.
    async fn send_audio(&mut self, frame: &AudioFrame) -> Result<(), SttFailure>;
    /// Send `{"type":"CloseStream"}`. Must be called only after the last frame.
    /// The final transcript then arrives as `SttEvent::Flushed`.
    async fn close_stream(&mut self) -> Result<(), SttFailure>;
    /// Drop the socket immediately (cancel / supersede). Never errors.
    fn abort(&mut self);
}

// ───────────────────────────── answer providers ─────────────────────────────

/// The prompt, split so providers can cache the stable prefix. `Debug` shows
/// lengths only (the prefix carries resume/notes text).
#[derive(Clone, PartialEq, Eq)]
pub struct PromptParts {
    /// Call-type role + profile sections + grounding. Cache this.
    pub cached_prefix: String,
    /// Answer-style instruction. Flipping style must not invalidate the cache.
    pub style_suffix: String,
    pub user_message: String,
}

impl std::fmt::Debug for PromptParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptParts")
            .field("cached_prefix_len", &self.cached_prefix.len())
            .field("style_suffix_len", &self.style_suffix.len())
            .field("user_message_len", &self.user_message.len())
            .finish()
    }
}

/// A fully-built request, built ONCE per answer. A retry resends exactly these
/// bytes. `Debug` is redacted because headers carry the key.
#[derive(Clone)]
pub struct PreparedRequest {
    pub url: String,
    /// Header name -> value. Secret-bearing values are `HeaderValue::Secret`.
    pub headers: Vec<(String, HeaderValue)>,
    pub body: bytes::Bytes,
}

#[derive(Clone)]
pub enum HeaderValue {
    Plain(String),
    Secret(String),
}

impl HeaderValue {
    pub fn as_str(&self) -> &str {
        match self {
            HeaderValue::Plain(s) | HeaderValue::Secret(s) => s,
        }
    }
}

impl std::fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    match v {
                        HeaderValue::Plain(s) => s.as_str(),
                        HeaderValue::Secret(_) => "***",
                    },
                )
            })
            .collect();
        f.debug_struct("PreparedRequest")
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailureKind {
    /// Connection-level failure BEFORE any byte of response (DNS, TLS, refused,
    /// reset during connect). The only kind that is retried (once, and only
    /// before any delta).
    Connect,
    /// Non-2xx status not covered below.
    Http { status: u16 },
    /// 401 / 403.
    Auth { status: u16 },
    /// 429 / 529 / in-stream overloaded or rate-limit error.
    RateLimit { status: u16 },
    /// Stream broke after the response started.
    StreamDrop,
    /// Aborted by the caller.
    Aborted,
    /// In-stream error payload from the provider.
    ProviderError,
    /// EOF without the provider's real terminal signal.
    Incomplete,
    /// Terminal signal arrived but the answer is empty/whitespace.
    EmptyAnswer,
    /// 404 / model_decommissioned / model_not_found.
    ModelUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProviderFailure {
    pub kind: ProviderFailureKind,
    /// Actionable user copy: status code quoted, <=200-char provider snippet,
    /// never a stack trace, never the key.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamOutcome {
    pub finish: Finish,
    /// Concatenation of every delta sent.
    pub answer: String,
}

#[async_trait]
pub trait AnswerProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Secret id this provider needs, e.g. "anthropic".
    fn key_id(&self) -> &'static str;
    fn model(&self) -> &'static str;

    /// Build the request once. Pure; no I/O.
    fn build_request(&self, prompt: &PromptParts, key: &Secret) -> PreparedRequest;

    /// Send `req` and stream text deltas into `deltas` (in order, nothing
    /// inserted between content blocks). Returns only when the provider's real
    /// terminal signal was seen, or with a failure. The caller owns the
    /// first-token / total watchdogs (it drops this future to abort) and the
    /// retry policy.
    async fn stream(
        &self,
        req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure>;

    /// Throttled (2 s), fire-and-forget warm-up of a pooled TLS connection to
    /// the provider origin. Never blocks, never errors.
    fn prewarm(&self);
}

/// Lookup by provider id.
pub trait ProviderRegistry: Send + Sync {
    fn get(&self, id: &str) -> Option<Arc<dyn AnswerProvider>>;
    /// The default provider ("anthropic").
    fn default_provider(&self) -> Arc<dyn AnswerProvider>;
    fn all(&self) -> Vec<Arc<dyn AnswerProvider>>;
}

// ───────────────────────────── settings ─────────────────────────────

/// Everything the session needs to build an answer. Read from memory only
/// (never touches disk), so it is cheap to call on the runtime thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerConfig {
    pub profile: Profile,
    pub call_type: CallType,
    pub style: AnswerStyle,
    pub provider_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SecretReadError(pub String);

pub trait SettingsReader: Send + Sync {
    /// Memory-only snapshot of the active profile/style/provider.
    fn answer_config(&self) -> AnswerConfig;
    /// Decrypt a stored secret. MAY BLOCK (DPAPI) — call via spawn_blocking.
    /// Undecryptable values read as `Ok(None)`.
    fn get_secret(&self, key_id: &str) -> Result<Option<Secret>, SecretReadError>;
}

// ───────────────────────────── misc ─────────────────────────────

/// Sink for core -> page events. The shell's pump assigns `seq`, preserves
/// order, never drops must-deliver events, and coalesces audio levels.
/// `emit` must not block.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: CoreEvent);
}

/// Wall clock for epoch-ms values sent to the page (`deadlineMs`). Durations
/// and timers use `tokio::time` so tests can pause/advance time.
pub trait Clock: Send + Sync {
    fn epoch_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn epoch_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct KeystoreError(pub String);

/// Secret encryption at rest (DPAPI, current-user scope, in production).
/// MUST fail closed: an error means "do not store", never "store plaintext".
pub trait Keystore: Send + Sync {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, KeystoreError>;
    fn unprotect(&self, blob: &[u8]) -> Result<Vec<u8>, KeystoreError>;
}

/// Screen-capture exclusion for the main window.
pub trait DisplayAffinity: Send + Sync {
    /// Apply WDA_EXCLUDEFROMCAPTURE (0x11).
    fn apply(&self) -> Result<(), String>;
    /// Read back via GetWindowDisplayAffinity; true only if it is 0x11.
    fn verify(&self) -> Result<bool, String>;
}

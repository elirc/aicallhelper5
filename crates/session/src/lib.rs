//! The session actor. PUBLIC API IS PINNED (the shell is written against it) —
//! the session agent implements the bodies and may add private modules, but
//! must not change these signatures without telling the orchestrator.
//!
//! ONE actor task owns all session state (single live-session slot, phase,
//! flags, command tickets, timers). `SessionHandle` methods send messages to it.
//!
//! Design notes (see also `docs/testing/session.md`):
//! * Per-session work (STT connect, socket writer/reader, audio relay, provider
//!   stream) runs in child tasks that report back tagged with the session's
//!   generation; the actor ignores anything stale.
//! * All audio calls go through ONE lane task, in the order the actor queued
//!   them, so a superseded session's discard always precedes the next start.
//! * A superseded or cancelled session emits NOTHING further (no terminal
//!   event): the page initiated the new command / the cancel and already
//!   moved on. Every other session ends with exactly one `llm:done` XOR
//!   `session:error`.
//! * A device-open failure ends the session at once with the device error
//!   (`internal` + device copy), so it can never be reported as `no_speech`.
//! * Device loss emits `audio:device{lost}` and then follows the stop path
//!   (no `session:autostopped`, which is reserved for the 120 s cap).

mod actor;
mod errors;
mod tasks;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use callcore_contract::config::SessionTimeouts;
use callcore_contract::ports::{
    AnswerProvider, AudioSource, Clock, EventSink, ProviderRegistry, SettingsReader, SttConnector,
};
use callcore_contract::{copy, AppError, ErrorCode, Secret, SessionId, SessionStatus};
use tokio::sync::{mpsc, oneshot, watch};

use crate::actor::{ActorDeps, Cmd, InstallKind};
pub use crate::errors::{provider_failure_to_app_error, stt_failure_to_app_error};

/// Error copy once the actor has stopped.
pub const SHUTTING_DOWN: &str = "The session core is shutting down.";
/// Secret id of the Deepgram key.
pub const DEEPGRAM_KEY_ID: &str = "deepgram";

pub struct SessionDeps {
    pub audio: Arc<dyn AudioSource>,
    pub stt: Arc<dyn SttConnector>,
    pub providers: Arc<dyn ProviderRegistry>,
    pub settings: Arc<dyn SettingsReader>,
    pub sink: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub timeouts: SessionTimeouts,
}

struct Inner {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    /// Monotonic command tickets for start/ask (claimed before any await).
    tickets: Arc<AtomicU64>,
    providers: Arc<dyn ProviderRegistry>,
    settings: Arc<dyn SettingsReader>,
}

/// Cheap to clone; all clones talk to the same actor.
#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Inner>,
    status_rx: watch::Receiver<SessionStatus>,
}

fn shutting_down() -> AppError {
    AppError::internal(SHUTTING_DOWN)
}

impl SessionHandle {
    /// Spawn the actor on the current tokio runtime.
    pub fn spawn(deps: SessionDeps) -> Self {
        let (status_tx, status_rx) = watch::channel(SessionStatus::idle());
        let tickets = Arc::new(AtomicU64::new(0));
        let cmd_tx = actor::spawn(ActorDeps {
            audio: deps.audio,
            stt: deps.stt,
            settings: deps.settings.clone(),
            sink: deps.sink,
            clock: deps.clock,
            timeouts: deps.timeouts,
            tickets: tickets.clone(),
            status_tx,
        });
        Self {
            inner: Arc::new(Inner {
                cmd_tx,
                tickets,
                providers: deps.providers,
                settings: deps.settings,
            }),
            status_rx,
        }
    }

    /// Record. Returns the new session id ("s1", "s2", …). Supersedes any live
    /// session. Errors: `no_stt_key`, `no_llm_key`, `aborted` (lost the race to
    /// a newer command), `internal`.
    pub async fn start_session(&self) -> Result<SessionId, AppError> {
        // Claimed before any await.
        let ticket = self.claim_ticket();
        let keys = self.read_keys(true).await?;
        let stt_key = keys
            .stt_key
            .ok_or_else(|| AppError::new(ErrorCode::NoSttKey, copy::NO_STT_KEY))?;
        let llm_key = keys
            .llm_key
            .ok_or_else(|| AppError::new(ErrorCode::NoLlmKey, copy::NO_LLM_KEY))?;
        self.install(
            ticket,
            InstallKind::Record { stt_key },
            keys.provider,
            llm_key,
        )
        .await
    }

    /// Stop & Answer. Ok only for the live, not-yet-stopping session (including
    /// one still connecting); otherwise `Err(internal, copy::STOP_NOT_TAKEN)`.
    pub async fn stop_session(&self, id: &SessionId) -> Result<(), AppError> {
        let (reply, rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .send(Cmd::Stop {
                id: id.clone(),
                reply,
            })
            .map_err(|_| shutting_down())?;
        rx.await.unwrap_or_else(|_| Err(shutting_down()))
    }

    /// Typed question. Validates/trims FIRST (empty -> error, live session
    /// untouched), then supersedes and answers directly.
    pub async fn ask(&self, text: String) -> Result<SessionId, AppError> {
        let question = text.trim();
        if question.is_empty() {
            return Err(AppError::internal(copy::EMPTY_QUESTION));
        }
        let question = question.to_string();
        let ticket = self.claim_ticket();
        let keys = self.read_keys(false).await?;
        let llm_key = keys
            .llm_key
            .ok_or_else(|| AppError::new(ErrorCode::NoLlmKey, copy::NO_LLM_KEY))?;
        self.install(
            ticket,
            InstallKind::Ask { question },
            keys.provider,
            llm_key,
        )
        .await
    }

    /// Fire-and-forget, idempotent, never errors, affects only `id`.
    pub fn cancel_session(&self, id: &SessionId) {
        let _ = self.inner.cmd_tx.send(Cmd::Cancel { id: id.clone() });
    }

    /// Latest status, never blocks (read from the watch channel).
    pub fn status(&self) -> SessionStatus {
        self.status_rx.borrow().clone()
    }

    /// Changes whenever phase/id/recording changes (the shell bumps its
    /// snapshot revision on every change).
    pub fn status_watch(&self) -> watch::Receiver<SessionStatus> {
        self.status_rx.clone()
    }

    /// Cancel the live session (discard-stop audio) and stop the actor.
    pub async fn shutdown(&self) {
        let (reply, rx) = oneshot::channel();
        if self.inner.cmd_tx.send(Cmd::Shutdown { reply }).is_ok() {
            let _ = rx.await;
        }
    }

    // ───────────────────────────── helpers ─────────────────────────────

    fn claim_ticket(&self) -> u64 {
        self.inner.tickets.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Read the provider choice (memory) and the keys (DPAPI, off the runtime
    /// thread). Blank keys read as missing.
    async fn read_keys(&self, need_stt: bool) -> Result<Keys, AppError> {
        if self.inner.cmd_tx.is_closed() {
            return Err(shutting_down());
        }
        let config = self.inner.settings.answer_config();
        let registry = &self.inner.providers;
        let provider = registry
            .get(&config.provider_id)
            .unwrap_or_else(|| registry.default_provider());
        let settings = self.inner.settings.clone();
        let key_id = provider.key_id();
        let read = tokio::task::spawn_blocking(move || {
            let stt_key = if need_stt {
                settings.get_secret(DEEPGRAM_KEY_ID)?
            } else {
                None
            };
            let llm_key = settings.get_secret(key_id)?;
            Ok::<_, callcore_contract::ports::SecretReadError>((stt_key, llm_key))
        })
        .await;
        let (stt_key, llm_key) = match read {
            Ok(Ok(keys)) => keys,
            Ok(Err(e)) => return Err(AppError::internal(e.0)),
            Err(_) => return Err(AppError::internal(errors::KEY_READ_FAILED)),
        };
        let usable = |k: Option<Secret>| k.filter(|k| !k.is_blank());
        Ok(Keys {
            provider,
            stt_key: usable(stt_key),
            llm_key: usable(llm_key),
        })
    }

    async fn install(
        &self,
        ticket: u64,
        kind: InstallKind,
        provider: Arc<dyn AnswerProvider>,
        llm_key: Secret,
    ) -> Result<SessionId, AppError> {
        let (reply, rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .send(Cmd::Install {
                ticket,
                kind,
                provider,
                llm_key,
                reply,
            })
            .map_err(|_| shutting_down())?;
        rx.await.unwrap_or_else(|_| Err(shutting_down()))
    }
}

struct Keys {
    provider: Arc<dyn AnswerProvider>,
    stt_key: Option<Secret>,
    llm_key: Option<Secret>,
}

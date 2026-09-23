//! Scriptable fakes for every session port. All delays use `tokio::time`, so
//! `#[tokio::test(start_paused = true)]` tests are exact and never sleep for
//! real. No sockets, no devices, no network.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use callcore_contract::config::SessionTimeouts;
use callcore_contract::ports::{
    AnswerConfig, AnswerProvider, AudioError, AudioFrame, AudioMsg, AudioSource, Clock,
    DrainReport, EventSink, FrameSink, HeaderValue, PreparedRequest, PromptParts, ProviderFailure,
    ProviderFailureKind, ProviderRegistry, SecretReadError, SettingsReader, StreamOutcome,
    SttConnection, SttConnector, SttEvent, SttFailure, SttFailureKind, SttSender,
};
use callcore_contract::{
    AnswerStyle, CallType, CoreEvent, ErrorCode, Finish, Metrics, Profile, Secret, SessionId,
};
use callcore_session::{SessionDeps, SessionHandle};

#[allow(unused_imports)]
pub use callcore_contract::ports::Clock as _;
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

pub const DG_KEY: &str = "dg-secret-key-123";
pub const LLM_KEY: &str = "sk-ant-secret-key-456";
pub const RESUME: &str = "RESUME-TEXT-private-789";
pub const TRANSCRIPT: &str = "Tell me about a time you led a project.";

pub fn frame(tag: i16) -> AudioFrame {
    AudioFrame {
        samples: vec![tag; 8],
        rms: 0.25,
    }
}

pub fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

pub fn ms(m: u64) -> Duration {
    Duration::from_millis(m)
}

// ───────────────────────────── audio ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCall {
    Start,
    Discard,
    Drain,
}

#[derive(Default)]
struct AudioState {
    start_delay: Duration,
    start_results: VecDeque<Result<(), AudioError>>,
    drain_delay: Duration,
    drain_tail: Vec<AudioFrame>,
    sink: Option<FrameSink>,
    sinks: Vec<FrameSink>,
    calls: Vec<AudioCall>,
}

#[derive(Default)]
pub struct FakeAudio {
    st: Mutex<AudioState>,
}

impl FakeAudio {
    pub fn set_start_delay(&self, d: Duration) {
        self.st.lock().unwrap().start_delay = d;
    }
    pub fn fail_next_start(&self, e: AudioError) {
        self.st.lock().unwrap().start_results.push_back(Err(e));
    }
    /// The drain takes `delay`, then pushes `tail` (the final pre-stop audio).
    pub fn set_drain(&self, delay: Duration, tail: Vec<AudioFrame>) {
        let mut st = self.st.lock().unwrap();
        st.drain_delay = delay;
        st.drain_tail = tail;
    }
    /// Push into the CURRENT capture sink. False when capture is not running.
    pub fn push(&self, msg: AudioMsg) -> bool {
        let st = self.st.lock().unwrap();
        st.sink.as_ref().is_some_and(|s| s.send(msg).is_ok())
    }
    pub fn push_frame(&self, tag: i16) -> bool {
        self.push(AudioMsg::Frame(frame(tag)))
    }
    /// Push into the sink of the n-th successful start (even after it was
    /// cleared) — simulates a late callback.
    pub fn push_to_old_sink(&self, n: usize, msg: AudioMsg) -> bool {
        let st = self.st.lock().unwrap();
        st.sinks.get(n).is_some_and(|s| s.send(msg).is_ok())
    }
    pub fn calls(&self) -> Vec<AudioCall> {
        self.st.lock().unwrap().calls.clone()
    }
    pub fn capturing(&self) -> bool {
        self.st.lock().unwrap().sink.is_some()
    }
    /// True when the session side of every sink ever handed out is gone.
    pub fn all_receivers_dropped(&self) -> bool {
        self.st.lock().unwrap().sinks.iter().all(|s| s.is_closed())
    }
}

#[async_trait]
impl AudioSource for FakeAudio {
    async fn start(&self, sink: FrameSink) -> Result<(), AudioError> {
        let delay = {
            let mut st = self.st.lock().unwrap();
            st.calls.push(AudioCall::Start);
            st.start_delay
        };
        tokio::time::sleep(delay).await;
        let mut st = self.st.lock().unwrap();
        match st.start_results.pop_front().unwrap_or(Ok(())) {
            Ok(()) => {
                st.sinks.push(sink.clone());
                st.sink = Some(sink);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn stop_discard(&self) {
        let mut st = self.st.lock().unwrap();
        st.calls.push(AudioCall::Discard);
        st.sink = None;
    }

    async fn stop_and_drain(&self, timeout: Duration) -> Result<DrainReport, AudioError> {
        let delay = {
            let mut st = self.st.lock().unwrap();
            st.calls.push(AudioCall::Drain);
            st.drain_delay
        };
        tokio::time::sleep(delay.min(timeout)).await;
        let mut st = self.st.lock().unwrap();
        let tail = std::mem::take(&mut st.drain_tail);
        let n = tail.len();
        if let Some(sink) = st.sink.take() {
            for f in tail {
                let _ = sink.send(AudioMsg::Frame(f));
            }
        }
        Ok(DrainReport {
            frames_flushed: n,
            timed_out: delay > timeout,
        })
    }
}

// ───────────────────────────── STT ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttLog {
    /// First sample of the frame (the test's tag).
    Frame(i16),
    Close,
    Abort,
    Dropped,
}

#[derive(Clone)]
pub enum FlushMode {
    /// On CloseStream: after `delay`, send `Flushed{transcript}`, then
    /// optionally a late `Failed` (must be ignored by the session).
    Auto {
        delay: Duration,
        transcript: String,
        late_failure: Option<SttFailure>,
    },
    /// Never flush on its own (the test drives events).
    Manual,
}

struct SttState {
    connect_delay: Duration,
    connect_results: VecDeque<Result<(), SttFailure>>,
    connects: usize,
    keys: Vec<String>,
    log: Vec<(usize, SttLog)>,
    events_tx: Option<mpsc::Sender<SttEvent>>,
    all_events_tx: Vec<mpsc::Sender<SttEvent>>,
    flush: FlushMode,
    fail_next_send: Option<SttFailure>,
}

pub struct FakeStt {
    st: Arc<Mutex<SttState>>,
}

impl Default for FakeStt {
    fn default() -> Self {
        Self {
            st: Arc::new(Mutex::new(SttState {
                connect_delay: Duration::ZERO,
                connect_results: VecDeque::new(),
                connects: 0,
                keys: Vec::new(),
                log: Vec::new(),
                events_tx: None,
                all_events_tx: Vec::new(),
                flush: FlushMode::Auto {
                    delay: Duration::ZERO,
                    transcript: TRANSCRIPT.into(),
                    late_failure: None,
                },
                fail_next_send: None,
            })),
        }
    }
}

impl FakeStt {
    pub fn set_connect_delay(&self, d: Duration) {
        self.st.lock().unwrap().connect_delay = d;
    }
    pub fn fail_next_connect(&self, f: SttFailure) {
        self.st.lock().unwrap().connect_results.push_back(Err(f));
    }
    pub fn fail_next_send(&self, f: SttFailure) {
        self.st.lock().unwrap().fail_next_send = Some(f);
    }
    pub fn set_flush(&self, mode: FlushMode) {
        self.st.lock().unwrap().flush = mode;
    }
    pub fn set_flush_transcript(&self, text: &str) {
        self.set_flush(FlushMode::Auto {
            delay: Duration::ZERO,
            transcript: text.into(),
            late_failure: None,
        });
    }
    pub fn connects(&self) -> usize {
        self.st.lock().unwrap().connects
    }
    pub fn keys(&self) -> Vec<String> {
        self.st.lock().unwrap().keys.clone()
    }
    /// Log of connection `conn` (0-based).
    pub fn log(&self, conn: usize) -> Vec<SttLog> {
        self.st
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|(c, _)| *c == conn)
            .map(|(_, l)| l.clone())
            .collect()
    }
    pub fn frames_sent(&self, conn: usize) -> Vec<i16> {
        self.log(conn)
            .into_iter()
            .filter_map(|l| {
                if let SttLog::Frame(t) = l {
                    Some(t)
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn connected(&self) -> bool {
        self.st.lock().unwrap().events_tx.is_some()
    }
    /// Emit an event on the CURRENT connection.
    pub fn emit(&self, ev: SttEvent) -> bool {
        let tx = self.st.lock().unwrap().events_tx.clone();
        tx.is_some_and(|tx| tx.try_send(ev).is_ok())
    }
    /// Emit on connection `conn` even if it is stale.
    pub fn emit_on(&self, conn: usize, ev: SttEvent) -> bool {
        let tx = self.st.lock().unwrap().all_events_tx.get(conn).cloned();
        tx.is_some_and(|tx| tx.try_send(ev).is_ok())
    }
    pub fn transcript(&self, text: &str, is_final: bool) -> bool {
        self.emit(SttEvent::Transcript {
            text: text.into(),
            is_final,
        })
    }
    /// Drop every event sender: the session's reader sees the stream end
    /// without `Flushed`/`Failed`.
    pub fn end_events(&self) {
        let mut st = self.st.lock().unwrap();
        st.events_tx = None;
        st.all_events_tx.clear();
    }
    /// True when every connection's event receiver is gone (no reader task).
    pub fn all_readers_dropped(&self) -> bool {
        self.st
            .lock()
            .unwrap()
            .all_events_tx
            .iter()
            .all(|t| t.is_closed())
    }
}

#[async_trait]
impl SttConnector for FakeStt {
    async fn connect(&self, key: &Secret) -> Result<SttConnection, SttFailure> {
        let (delay, conn) = {
            let mut st = self.st.lock().unwrap();
            st.keys.push(key.expose().to_string());
            (st.connect_delay, st.connects)
        };
        tokio::time::sleep(delay).await;
        let mut st = self.st.lock().unwrap();
        if let Some(Err(f)) = st.connect_results.pop_front() {
            return Err(f);
        }
        st.connects += 1;
        let (tx, rx) = mpsc::channel(256);
        st.events_tx = Some(tx.clone());
        st.all_events_tx.push(tx);
        Ok(SttConnection {
            sender: Box::new(FakeSttSender {
                st: self.st.clone(),
                conn,
            }),
            events: rx,
        })
    }
}

struct FakeSttSender {
    st: Arc<Mutex<SttState>>,
    conn: usize,
}

#[async_trait]
impl SttSender for FakeSttSender {
    async fn send_audio(&mut self, frame: &AudioFrame) -> Result<(), SttFailure> {
        let mut st = self.st.lock().unwrap();
        if let Some(f) = st.fail_next_send.take() {
            return Err(f);
        }
        st.log.push((self.conn, SttLog::Frame(frame.samples[0])));
        Ok(())
    }

    async fn close_stream(&mut self) -> Result<(), SttFailure> {
        let mut st = self.st.lock().unwrap();
        st.log.push((self.conn, SttLog::Close));
        if let FlushMode::Auto {
            delay,
            transcript,
            late_failure,
        } = st.flush.clone()
        {
            if let Some(tx) = st.all_events_tx.get(self.conn).cloned() {
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = tx.send(SttEvent::Flushed { transcript }).await;
                    if let Some(f) = late_failure {
                        let _ = tx.send(SttEvent::Failed(f)).await;
                    }
                });
            }
        }
        Ok(())
    }

    fn abort(&mut self) {
        self.st.lock().unwrap().log.push((self.conn, SttLog::Abort));
    }
}

impl Drop for FakeSttSender {
    fn drop(&mut self) {
        if let Ok(mut st) = self.st.lock() {
            st.log.push((self.conn, SttLog::Dropped));
        }
    }
}

// ───────────────────────────── provider ─────────────────────────────

#[derive(Clone)]
pub enum Step {
    Sleep(Duration),
    Delta(String),
    Return(Result<StreamOutcome, ProviderFailure>),
    /// Never returns (only a watchdog or cancel ends it).
    Hang,
}

#[derive(Clone)]
pub struct Script(pub Vec<Step>);

impl Script {
    /// Deltas, then success with their concatenation.
    pub fn ok(deltas: &[&str]) -> Self {
        Self::ok_after(Duration::ZERO, deltas)
    }
    /// Wait `first` before the first delta, then stream and succeed.
    pub fn ok_after(first: Duration, deltas: &[&str]) -> Self {
        let mut steps = vec![Step::Sleep(first)];
        steps.extend(deltas.iter().map(|d| Step::Delta((*d).into())));
        steps.push(Step::Return(Ok(StreamOutcome {
            finish: Finish::Complete,
            answer: deltas.concat(),
        })));
        Script(steps)
    }
    pub fn fail(kind: ProviderFailureKind, message: &str) -> Self {
        Script(vec![Step::Return(Err(ProviderFailure {
            kind,
            message: message.into(),
        }))])
    }
}

struct ProvState {
    scripts: VecDeque<Script>,
    default: Script,
    stream_calls: usize,
    builds: usize,
    prewarms: usize,
    requests: Vec<bytes::Bytes>,
    prompts: Vec<PromptParts>,
    keys: Vec<String>,
    delta_txs: Vec<mpsc::UnboundedSender<String>>,
}

pub struct FakeProvider {
    id: &'static str,
    key_id: &'static str,
    st: Mutex<ProvState>,
    /// Number of `stream` futures currently alive.
    live_streams: Arc<AtomicUsize>,
}

pub const DEFAULT_ANSWER: [&str; 3] = ["I led ", "the migration ", "to Rust."];

impl FakeProvider {
    pub fn new(id: &'static str, key_id: &'static str) -> Self {
        Self {
            id,
            key_id,
            st: Mutex::new(ProvState {
                scripts: VecDeque::new(),
                default: Script::ok(&DEFAULT_ANSWER),
                stream_calls: 0,
                builds: 0,
                prewarms: 0,
                requests: Vec::new(),
                prompts: Vec::new(),
                keys: Vec::new(),
                delta_txs: Vec::new(),
            }),
            live_streams: Arc::new(AtomicUsize::new(0)),
        }
    }
    /// Script for the next `stream` call (FIFO); afterwards the default runs.
    pub fn push_script(&self, s: Script) {
        self.st.lock().unwrap().scripts.push_back(s);
    }
    pub fn set_default(&self, s: Script) {
        self.st.lock().unwrap().default = s;
    }
    pub fn stream_calls(&self) -> usize {
        self.st.lock().unwrap().stream_calls
    }
    pub fn builds(&self) -> usize {
        self.st.lock().unwrap().builds
    }
    pub fn prewarms(&self) -> usize {
        self.st.lock().unwrap().prewarms
    }
    pub fn requests(&self) -> Vec<bytes::Bytes> {
        self.st.lock().unwrap().requests.clone()
    }
    pub fn prompts(&self) -> Vec<PromptParts> {
        self.st.lock().unwrap().prompts.clone()
    }
    pub fn keys(&self) -> Vec<String> {
        self.st.lock().unwrap().keys.clone()
    }
    pub fn live_streams(&self) -> usize {
        self.live_streams.load(Ordering::SeqCst)
    }
    /// True when no session task still holds a delta receiver.
    pub fn all_delta_receivers_dropped(&self) -> bool {
        self.st
            .lock()
            .unwrap()
            .delta_txs
            .iter()
            .all(|t| t.is_closed())
    }
}

struct StreamGuard(Arc<AtomicUsize>);
impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl AnswerProvider for FakeProvider {
    fn id(&self) -> &'static str {
        self.id
    }
    fn display_name(&self) -> &'static str {
        "Fake"
    }
    fn key_id(&self) -> &'static str {
        self.key_id
    }
    fn model(&self) -> &'static str {
        "fake-model"
    }

    fn build_request(&self, prompt: &PromptParts, key: &Secret) -> PreparedRequest {
        let mut st = self.st.lock().unwrap();
        st.builds += 1;
        st.prompts.push(prompt.clone());
        // The build counter makes a rebuilt request differ from the original.
        let body = format!(
            "{}|{}|{}|build#{}",
            prompt.cached_prefix, prompt.style_suffix, prompt.user_message, st.builds
        );
        PreparedRequest {
            url: format!("https://{}.invalid/v1", self.id),
            headers: vec![
                (
                    "x-api-key".into(),
                    HeaderValue::Secret(key.expose().to_string()),
                ),
                (
                    "content-type".into(),
                    HeaderValue::Plain("application/json".into()),
                ),
            ],
            body: bytes::Bytes::from(body),
        }
    }

    async fn stream(
        &self,
        req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure> {
        self.live_streams.fetch_add(1, Ordering::SeqCst);
        let _guard = StreamGuard(self.live_streams.clone());
        let script = {
            let mut st = self.st.lock().unwrap();
            st.stream_calls += 1;
            st.requests.push(req.body.clone());
            if let Some((_, v)) = req.headers.iter().find(|(k, _)| k == "x-api-key") {
                st.keys.push(v.as_str().to_string());
            }
            st.delta_txs.push(deltas.clone());
            st.scripts.pop_front().unwrap_or_else(|| st.default.clone())
        };
        let mut sent = String::new();
        for step in script.0 {
            match step {
                Step::Sleep(d) => tokio::time::sleep(d).await,
                Step::Delta(d) => {
                    sent.push_str(&d);
                    let _ = deltas.send(d);
                }
                Step::Return(r) => return r,
                Step::Hang => std::future::pending::<()>().await,
            }
        }
        Ok(StreamOutcome {
            finish: Finish::Complete,
            answer: sent,
        })
    }

    fn prewarm(&self) {
        self.st.lock().unwrap().prewarms += 1;
    }
}

pub struct FakeRegistry {
    pub providers: Vec<Arc<FakeProvider>>,
}

impl ProviderRegistry for FakeRegistry {
    fn get(&self, id: &str) -> Option<Arc<dyn AnswerProvider>> {
        self.providers
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.clone() as Arc<dyn AnswerProvider>)
    }
    fn default_provider(&self) -> Arc<dyn AnswerProvider> {
        self.providers[0].clone()
    }
    fn all(&self) -> Vec<Arc<dyn AnswerProvider>> {
        self.providers
            .iter()
            .map(|p| p.clone() as Arc<dyn AnswerProvider>)
            .collect()
    }
}

// ───────────────────────────── settings ─────────────────────────────

#[derive(Default)]
struct GateState {
    armed: bool,
    blocked: bool,
    released: bool,
}

pub struct FakeSettings {
    config: Mutex<AnswerConfig>,
    secrets: Mutex<HashMap<String, String>>,
    read_error: Mutex<Option<String>>,
    reads: AtomicUsize,
    gate: Mutex<GateState>,
    gate_cv: Condvar,
}

pub fn default_profile() -> Profile {
    Profile {
        id: "default".into(),
        name: "Default".into(),
        call_type: CallType::Behavioral,
        focus: "Rust".into(),
        resume: RESUME.into(),
        job_description: "Staff engineer".into(),
        notes: String::new(),
    }
}

impl Default for FakeSettings {
    fn default() -> Self {
        let mut secrets = HashMap::new();
        secrets.insert("deepgram".to_string(), DG_KEY.to_string());
        secrets.insert("anthropic".to_string(), LLM_KEY.to_string());
        Self {
            config: Mutex::new(AnswerConfig {
                profile: default_profile(),
                call_type: CallType::Behavioral,
                style: AnswerStyle::Balanced,
                provider_id: "anthropic".into(),
            }),
            secrets: Mutex::new(secrets),
            read_error: Mutex::new(None),
            reads: AtomicUsize::new(0),
            gate: Mutex::new(GateState::default()),
            gate_cv: Condvar::new(),
        }
    }
}

impl FakeSettings {
    pub fn set_secret(&self, id: &str, value: Option<&str>) {
        let mut s = self.secrets.lock().unwrap();
        match value {
            Some(v) => s.insert(id.into(), v.into()),
            None => s.remove(id),
        };
    }
    pub fn set_read_error(&self, msg: Option<&str>) {
        *self.read_error.lock().unwrap() = msg.map(String::from);
    }
    pub fn update_config(&self, f: impl FnOnce(&mut AnswerConfig)) {
        f(&mut self.config.lock().unwrap());
    }
    pub fn config(&self) -> AnswerConfig {
        self.config.lock().unwrap().clone()
    }
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    /// The NEXT `get_secret` call blocks (on its blocking thread) until
    /// [`FakeSettings::release_gate`].
    pub fn arm_gate(&self) {
        let mut g = self.gate.lock().unwrap();
        g.armed = true;
        g.released = false;
    }
    pub fn gate_blocked(&self) -> bool {
        self.gate.lock().unwrap().blocked
    }
    pub fn release_gate(&self) {
        let mut g = self.gate.lock().unwrap();
        g.released = true;
        self.gate_cv.notify_all();
    }
}

impl SettingsReader for FakeSettings {
    fn answer_config(&self) -> AnswerConfig {
        self.config.lock().unwrap().clone()
    }

    fn get_secret(&self, key_id: &str) -> Result<Option<Secret>, SecretReadError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        {
            let mut g = self.gate.lock().unwrap();
            if g.armed {
                g.armed = false;
                g.blocked = true;
                while !g.released {
                    g = self.gate_cv.wait(g).unwrap();
                }
                g.blocked = false;
            }
        }
        if let Some(e) = self.read_error.lock().unwrap().clone() {
            return Err(SecretReadError(e));
        }
        Ok(self
            .secrets
            .lock()
            .unwrap()
            .get(key_id)
            .map(|v| Secret::new(v.clone())))
    }
}

// ───────────────────────────── sink + clock ─────────────────────────────

#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<CoreEvent>>,
    notify: Notify,
}

/// Bound for `wait_for` in virtual (paused) time.
pub const WAIT_BOUND: Duration = Duration::from_secs(600);

pub fn kind(e: &CoreEvent) -> &'static str {
    match e {
        CoreEvent::SessionRecording { .. } => "session:recording",
        CoreEvent::SttPartial { .. } => "stt:partial",
        CoreEvent::AudioLevel { .. } => "audio:level",
        CoreEvent::AudioDevice { .. } => "audio:device",
        CoreEvent::SessionAutostopped { .. } => "session:autostopped",
        CoreEvent::LlmDelta { .. } => "llm:delta",
        CoreEvent::LlmDone { .. } => "llm:done",
        CoreEvent::SessionError { .. } => "session:error",
        _ => "other",
    }
}

pub fn is_terminal(e: &CoreEvent) -> bool {
    matches!(
        e,
        CoreEvent::LlmDone { .. } | CoreEvent::SessionError { .. }
    )
}

impl EventSink for RecordingSink {
    fn emit(&self, event: CoreEvent) {
        self.events.lock().unwrap().push(event);
        self.notify.notify_waiters();
    }
}

impl RecordingSink {
    pub fn events(&self) -> Vec<CoreEvent> {
        self.events.lock().unwrap().clone()
    }
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }
    pub fn for_session(&self, id: &SessionId) -> Vec<CoreEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.session_id() == Some(id))
            .collect()
    }
    pub fn kinds(&self, id: &SessionId) -> Vec<&'static str> {
        self.for_session(id).iter().map(kind).collect()
    }
    /// Kinds with consecutive duplicates collapsed (levels/partials/deltas vary in count).
    pub fn kinds_dedup(&self, id: &SessionId) -> Vec<&'static str> {
        let mut k = self.kinds(id);
        k.dedup();
        k
    }
    pub fn terminals(&self, id: &SessionId) -> Vec<CoreEvent> {
        self.for_session(id)
            .into_iter()
            .filter(is_terminal)
            .collect()
    }
    pub fn has_terminal(&self, id: &SessionId) -> bool {
        !self.terminals(id).is_empty()
    }

    /// Wait (letting virtual time pass) until `pred` holds; panics after
    /// `bound`.
    pub async fn wait_for_within(
        &self,
        bound: Duration,
        what: &str,
        pred: impl Fn(&[CoreEvent]) -> bool,
    ) {
        let deadline = Instant::now() + bound;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if pred(&self.events.lock().unwrap()) {
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("timed out waiting for {what}; events: {:#?}", self.events());
            }
        }
    }

    pub async fn wait_for(&self, what: &str, pred: impl Fn(&[CoreEvent]) -> bool) {
        self.wait_for_within(WAIT_BOUND, what, pred).await;
    }

    pub async fn wait_kind(&self, id: &SessionId, k: &'static str) {
        let id = id.clone();
        self.wait_for(k, move |evs| {
            evs.iter()
                .any(|e| e.session_id() == Some(&id) && kind(e) == k)
        })
        .await;
    }

    pub async fn wait_terminal(&self, id: &SessionId) -> CoreEvent {
        let id2 = id.clone();
        self.wait_for("terminal event", move |evs| {
            evs.iter()
                .any(|e| e.session_id() == Some(&id2) && is_terminal(e))
        })
        .await;
        self.terminals(id).remove(0)
    }

    pub async fn wait_error(&self, id: &SessionId) -> (ErrorCode, String) {
        match self.wait_terminal(id).await {
            CoreEvent::SessionError { error, .. } => (error.code, error.message),
            other => panic!("expected session:error, got {other:?}"),
        }
    }

    pub async fn wait_done(&self, id: &SessionId) -> Done {
        match self.wait_terminal(id).await {
            CoreEvent::LlmDone {
                transcript,
                answer,
                finish,
                call_type,
                metrics,
                ..
            } => Done {
                transcript,
                answer,
                finish,
                call_type,
                metrics,
            },
            other => panic!("expected llm:done, got {other:?}"),
        }
    }
}

#[derive(Debug)]
pub struct Done {
    pub transcript: String,
    pub answer: String,
    pub finish: Finish,
    pub call_type: CallType,
    pub metrics: Metrics,
}

/// Epoch clock driven by tokio's (pausable) clock.
pub struct FakeClock {
    base: u64,
    start: Instant,
}

pub const EPOCH_BASE: u64 = 1_800_000_000_000;

impl FakeClock {
    pub fn new() -> Self {
        Self {
            base: EPOCH_BASE,
            start: Instant::now(),
        }
    }
}

impl Clock for FakeClock {
    fn epoch_ms(&self) -> u64 {
        self.base + self.start.elapsed().as_millis() as u64
    }
}

// ───────────────────────────── harness ─────────────────────────────

pub struct Harness {
    pub handle: SessionHandle,
    pub audio: Arc<FakeAudio>,
    pub stt: Arc<FakeStt>,
    pub provider: Arc<FakeProvider>,
    pub groq: Arc<FakeProvider>,
    pub settings: Arc<FakeSettings>,
    pub sink: Arc<RecordingSink>,
    pub clock: Arc<FakeClock>,
    pub timeouts: SessionTimeouts,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_timeouts(SessionTimeouts::default())
    }

    pub fn with_timeouts(timeouts: SessionTimeouts) -> Self {
        let audio = Arc::new(FakeAudio::default());
        let stt = Arc::new(FakeStt::default());
        let provider = Arc::new(FakeProvider::new("anthropic", "anthropic"));
        let groq = Arc::new(FakeProvider::new("groq", "groq"));
        let settings = Arc::new(FakeSettings::default());
        let sink = Arc::new(RecordingSink::default());
        let clock = Arc::new(FakeClock::new());
        let handle = SessionHandle::spawn(SessionDeps {
            audio: audio.clone(),
            stt: stt.clone(),
            providers: Arc::new(FakeRegistry {
                providers: vec![provider.clone(), groq.clone()],
            }),
            settings: settings.clone(),
            sink: sink.clone(),
            clock: clock.clone(),
            timeouts: timeouts.clone(),
        });
        Self {
            handle,
            audio,
            stt,
            provider,
            groq,
            settings,
            sink,
            clock,
            timeouts,
        }
    }

    /// start_session + wait until capture is running (session:recording) and
    /// the STT socket is open.
    pub async fn start_recording(&self) -> SessionId {
        let id = self.handle.start_session().await.expect("start_session");
        self.sink.wait_kind(&id, "session:recording").await;
        self.wait_until("stt connected", || self.stt.connected())
            .await;
        id
    }

    /// Poll a non-event condition, yielding to other tasks (no time passes
    /// unless everything is idle, in which case virtual time advances 1 ms).
    pub async fn wait_until(&self, what: &str, cond: impl Fn() -> bool) {
        for _ in 0..100_000 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("condition never held: {what}");
    }

    /// Let every ready task run without moving the clock.
    pub async fn settle(&self) {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    pub fn stt_failure(kind: SttFailureKind, message: &str) -> SttFailure {
        SttFailure {
            kind,
            message: message.into(),
        }
    }
}

/// Every emitted event serialized, for leak checks.
pub fn serialized(events: &[CoreEvent]) -> String {
    events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

//! The session actor: the ONLY owner of session state. It processes external
//! commands, tagged child-task reports and its own deadlines one at a time.
//!
//! Timers are not tasks: each live session carries `Option<Instant>`
//! deadlines (cap, finalize, first-token, total) and the actor's select loop
//! sleeps until the earliest one. Dropping the session state therefore clears
//! every timer on every exit path (invariant 9).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use callcore_contract::config::{SessionTimeouts, PRE_CONNECT_BUFFER_FRAMES};
use callcore_contract::ports::{
    AnswerProvider, AudioFrame, AudioMsg, Clock, EventSink, PreparedRequest, SettingsReader,
    SttConnector, SttEvent,
};
use callcore_contract::{
    copy, AppError, CallType, CoreEvent, DeviceNoticeKind, ErrorCode, Metrics, Phase,
    RecordingInfo, Secret, SessionId, SessionStatus,
};
use callcore_prompt::{build_prompt, PromptInput};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, timeout, Instant};

use crate::errors::{
    audio_error_to_app_error, provider_failure_to_app_error, stt_failure_to_app_error,
    BUILD_FAILED, EMPTY_ANSWER, STT_CLOSED_EARLY,
};
use crate::tasks::{
    run_audio_lane, run_connect, run_forwarder, run_reader, run_stream, run_writer, AudioOp,
    ConnectError, InternalTx, SttLink, Tagged, TaskEvent, TaskKind, TaskSet, WriterCmd,
};

/// How long `shutdown` waits for the audio lane to finish its discard.
const LANE_EXIT_WAIT: Duration = Duration::from_secs(2);

pub(crate) enum Cmd {
    Install {
        ticket: u64,
        kind: InstallKind,
        provider: Arc<dyn AnswerProvider>,
        llm_key: Secret,
        reply: oneshot::Sender<Result<SessionId, AppError>>,
    },
    Stop {
        id: SessionId,
        reply: oneshot::Sender<Result<(), AppError>>,
    },
    Cancel {
        id: SessionId,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

pub(crate) enum InstallKind {
    Record { stt_key: Secret },
    Ask { question: String },
}

pub(crate) struct ActorDeps {
    pub audio: Arc<dyn callcore_contract::ports::AudioSource>,
    pub stt: Arc<dyn SttConnector>,
    pub settings: Arc<dyn SettingsReader>,
    pub sink: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub timeouts: SessionTimeouts,
    pub tickets: Arc<AtomicU64>,
    pub status_tx: watch::Sender<SessionStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Installed; audio start and STT connect in flight.
    Starting,
    /// Capture is running; the cap is armed.
    Recording,
    /// Stop accepted: waiting for socket / drain / flush.
    Finalizing,
    /// Provider request in flight.
    Answering,
}

impl Stage {
    fn phase(self) -> Phase {
        match self {
            Stage::Starting => Phase::Starting,
            Stage::Recording => Phase::Recording,
            Stage::Finalizing => Phase::Finalizing,
            Stage::Answering => Phase::Answering,
        }
    }
}

enum SttState {
    Connecting,
    Open {
        writer: mpsc::UnboundedSender<WriterCmd>,
    },
    /// Final transcript received (or not a recording session). Anything the
    /// STT side reports from now on is ignored (invariant 5).
    Done,
}

struct Live {
    gen: u64,
    id: SessionId,
    is_record: bool,
    stage: Stage,
    provider: Arc<dyn AnswerProvider>,
    llm_key: Secret,
    tasks: TaskSet,

    // ── recording ──
    recording: Option<RecordingInfo>,
    cap_deadline: Option<Instant>,
    stt: SttState,
    pre_buffer: VecDeque<AudioFrame>,
    /// False after the capture cutoff (drain complete).
    capture_open: bool,
    /// Handed to the audio lane with the Drain request (taken => drain issued).
    drain_tx: Option<oneshot::Sender<Instant>>,
    close_sent: bool,
    finalize_deadline: Option<Instant>,
    last_partial: String,

    // ── metrics clock ──
    /// Stop acceptance (or ask acceptance).
    accepted_at: Option<Instant>,
    drain_started: Option<Instant>,
    drain_done: Option<Instant>,
    flushed_at: Option<Instant>,

    // ── answer ──
    transcript: String,
    call_type: CallType,
    request: Option<Arc<PreparedRequest>>,
    attempt: u32,
    deltas: usize,
    first_delta_at: Option<Instant>,
    first_token_deadline: Option<Instant>,
    total_deadline: Option<Instant>,
}

impl Live {
    fn new(gen: u64, provider: Arc<dyn AnswerProvider>, llm_key: Secret, is_record: bool) -> Self {
        Self {
            gen,
            id: SessionId(format!("s{gen}")),
            is_record,
            stage: Stage::Starting,
            provider,
            llm_key,
            tasks: TaskSet::default(),
            recording: None,
            cap_deadline: None,
            stt: if is_record {
                SttState::Connecting
            } else {
                SttState::Done
            },
            pre_buffer: VecDeque::new(),
            capture_open: is_record,
            drain_tx: None,
            close_sent: false,
            finalize_deadline: None,
            last_partial: String::new(),
            accepted_at: None,
            drain_started: None,
            drain_done: None,
            flushed_at: None,
            transcript: String::new(),
            call_type: CallType::default(),
            request: None,
            attempt: 0,
            deltas: 0,
            first_delta_at: None,
            first_token_deadline: None,
            total_deadline: None,
        }
    }

    fn stopping(&self) -> bool {
        self.accepted_at.is_some()
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.cap_deadline,
            self.finalize_deadline,
            self.first_token_deadline,
            self.total_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn status(&self) -> SessionStatus {
        SessionStatus {
            id: Some(self.id.clone()),
            phase: self.stage.phase(),
            recording: if self.stage == Stage::Recording {
                self.recording
            } else {
                None
            },
        }
    }
}

pub(crate) struct Actor {
    stt: Arc<dyn SttConnector>,
    settings: Arc<dyn SettingsReader>,
    sink: Arc<dyn EventSink>,
    clock: Arc<dyn Clock>,
    timeouts: SessionTimeouts,
    tickets: Arc<AtomicU64>,
    status_tx: watch::Sender<SessionStatus>,
    internal_tx: InternalTx,
    lane_tx: mpsc::UnboundedSender<AudioOp>,
    lane: Option<JoinHandle<()>>,
    live_gen: Arc<AtomicU64>,
    next_gen: u64,
    live: Option<Live>,
}

/// Spawn the actor (and its audio lane); returns the command sender.
pub(crate) fn spawn(deps: ActorDeps) -> mpsc::UnboundedSender<Cmd> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (internal_tx, internal_rx) = mpsc::unbounded_channel();
    let (lane_tx, lane_rx) = mpsc::unbounded_channel();
    let live_gen = Arc::new(AtomicU64::new(0));
    let lane = tokio::spawn(run_audio_lane(
        deps.audio.clone(),
        lane_rx,
        internal_tx.clone(),
        live_gen.clone(),
        deps.timeouts.audio_drain,
    ));
    let actor = Actor {
        stt: deps.stt,
        settings: deps.settings,
        sink: deps.sink,
        clock: deps.clock,
        timeouts: deps.timeouts,
        tickets: deps.tickets,
        status_tx: deps.status_tx,
        internal_tx,
        lane_tx,
        lane: Some(lane),
        live_gen,
        next_gen: 0,
        live: None,
    };
    tokio::spawn(actor.run(cmd_rx, internal_rx));
    cmd_tx
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => sleep_until(d).await,
        None => std::future::pending().await,
    }
}

fn ms_between(from: Instant, to: Instant) -> u32 {
    u32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u32::MAX)
}

impl Actor {
    async fn run(
        mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
        mut internal_rx: mpsc::UnboundedReceiver<Tagged>,
    ) {
        loop {
            let deadline = self.live.as_ref().and_then(Live::next_deadline);
            tokio::select! {
                biased;
                Some(tagged) = internal_rx.recv() => self.on_task_event(tagged),
                cmd = cmd_rx.recv() => match cmd {
                    Some(Cmd::Shutdown { reply }) => {
                        self.shutdown().await;
                        let _ = reply.send(());
                        break;
                    }
                    Some(cmd) => self.on_cmd(cmd),
                    // Every handle dropped.
                    None => {
                        self.shutdown().await;
                        break;
                    }
                },
                () = sleep_until_opt(deadline) => self.on_deadline(),
            }
            self.publish();
        }
    }

    fn publish(&self) {
        let status = self
            .live
            .as_ref()
            .map_or_else(SessionStatus::idle, Live::status);
        self.status_tx.send_if_modified(|cur| {
            if *cur == status {
                false
            } else {
                *cur = status;
                true
            }
        });
    }

    fn emit(&self, event: CoreEvent) {
        self.sink.emit(event);
    }

    // ───────────────────────────── commands ─────────────────────────────

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Install {
                ticket,
                kind,
                provider,
                llm_key,
                reply,
            } => {
                let result = self.install(ticket, kind, provider, llm_key);
                self.publish();
                let _ = reply.send(result);
            }
            Cmd::Stop { id, reply } => {
                let result = self.stop(&id);
                self.publish();
                let _ = reply.send(result);
            }
            Cmd::Cancel { id } => {
                if self.live.as_ref().is_some_and(|l| l.id == id) {
                    tracing::debug!(session = %id, "cancelled");
                    self.discard_live();
                }
            }
            Cmd::Shutdown { .. } => unreachable!("handled in run"),
        }
    }

    fn install(
        &mut self,
        ticket: u64,
        kind: InstallKind,
        provider: Arc<dyn AnswerProvider>,
        llm_key: Secret,
    ) -> Result<SessionId, AppError> {
        // A newer start/ask was issued while this one read its keys: it must
        // never install over (or supersede) the newer command.
        if ticket != self.tickets.load(Ordering::SeqCst) {
            return Err(AppError::aborted());
        }
        // Supersede: the old session is discarded silently (no events).
        self.discard_live();

        self.next_gen += 1;
        let gen = self.next_gen;
        self.live_gen.store(gen, Ordering::SeqCst);
        let is_record = matches!(kind, InstallKind::Record { .. });
        let mut live = Live::new(gen, provider, llm_key, is_record);
        let id = live.id.clone();
        tracing::debug!(session = %id, record = is_record, "installed");
        live.provider.prewarm();

        match kind {
            InstallKind::Record { stt_key } => {
                let (sink, audio_rx) = mpsc::unbounded_channel::<AudioMsg>();
                let (drain_tx, drain_rx) = oneshot::channel();
                live.drain_tx = Some(drain_tx);
                live.tasks.push(
                    TaskKind::Forwarder,
                    tokio::spawn(run_forwarder(
                        gen,
                        audio_rx,
                        drain_rx,
                        self.internal_tx.clone(),
                    )),
                );
                let _ = self.lane_tx.send(AudioOp::Start { gen, sink });
                live.tasks.push(
                    TaskKind::Connect,
                    tokio::spawn(run_connect(
                        gen,
                        self.stt.clone(),
                        stt_key,
                        self.timeouts.stt_connect,
                        self.internal_tx.clone(),
                    )),
                );
                self.live = Some(live);
            }
            InstallKind::Ask { question } => {
                let now = Instant::now();
                live.accepted_at = Some(now);
                live.stage = Stage::Answering;
                self.live = Some(live);
                self.emit(CoreEvent::SttPartial {
                    session_id: id.clone(),
                    text: question.clone(),
                    is_final: true,
                });
                self.start_answer(question);
            }
        }
        Ok(id)
    }

    fn stop(&mut self, id: &SessionId) -> Result<(), AppError> {
        let taken = self
            .live
            .as_ref()
            .is_some_and(|l| &l.id == id && l.is_record && !l.stopping());
        if !taken {
            return Err(AppError::internal(copy::STOP_NOT_TAKEN));
        }
        self.begin_stop();
        Ok(())
    }

    /// Stop path shared by Stop, the record cap and device loss. The latency
    /// clock starts HERE, before the drain (lesson 5).
    fn begin_stop(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        live.accepted_at = Some(Instant::now());
        live.stage = Stage::Finalizing;
        live.recording = None;
        live.cap_deadline = None;
        live.provider.prewarm();
        tracing::debug!(session = %live.id, "stop accepted");
        self.maybe_drain();
    }

    /// Issue the drain once Stop was accepted AND the socket is open (a Stop
    /// during connect waits for the socket, so audioDrainMs excludes it).
    fn maybe_drain(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if !live.stopping() || !matches!(live.stt, SttState::Open { .. }) {
            return;
        }
        if let Some(done) = live.drain_tx.take() {
            let _ = self.lane_tx.send(AudioOp::Drain {
                gen: live.gen,
                done,
            });
        }
    }

    // ───────────────────────────── teardown ─────────────────────────────

    /// Drop the live session without any event (cancel / supersede /
    /// shutdown): discard-stop audio (never drain), abort the STT socket, drop
    /// the provider stream, clear its timers.
    fn discard_live(&mut self) {
        if let Some(live) = self.live.take() {
            self.release(live);
        }
    }

    fn release(&mut self, mut live: Live) {
        let _ = self
            .live_gen
            .compare_exchange(live.gen, 0, Ordering::SeqCst, Ordering::SeqCst);
        if live.is_record {
            let _ = self.lane_tx.send(AudioOp::Discard { gen: live.gen });
        }
        // Aborting the writer drops the GuardedSender => `abort()`; aborting
        // the stream task drops the provider future.
        live.tasks.abort_all();
    }

    /// Emit the session's single terminal event and tear it down.
    fn terminate(&mut self, event: CoreEvent) {
        if let Some(live) = self.live.take() {
            tracing::debug!(session = %live.id, "terminal");
            self.emit(event);
            self.release(live);
        }
    }

    fn fail(&mut self, error: AppError) {
        let Some(id) = self.live.as_ref().map(|l| l.id.clone()) else {
            return;
        };
        self.terminate(CoreEvent::SessionError {
            session_id: id,
            error,
        });
    }

    async fn shutdown(&mut self) {
        // Pending start/ask commands can never install now.
        self.tickets.fetch_add(1, Ordering::SeqCst);
        self.discard_live();
        let _ = self.lane_tx.send(AudioOp::Exit);
        if let Some(lane) = self.lane.take() {
            let _ = timeout(LANE_EXIT_WAIT, lane).await;
        }
        self.publish();
    }

    // ───────────────────────────── task reports ─────────────────────────────

    fn on_task_event(&mut self, tagged: Tagged) {
        let Tagged { gen, event } = tagged;
        if !self.live.as_ref().is_some_and(|l| l.gen == gen) {
            // Stale: a superseded/cancelled/finished session. Dropping the
            // payload drops any socket it carries (GuardedSender aborts).
            return;
        }
        match event {
            TaskEvent::AudioStarted(Ok(())) => self.on_audio_started(),
            TaskEvent::AudioStarted(Err(e)) => self.fail(audio_error_to_app_error(&e)),
            TaskEvent::Audio(msg) => self.on_audio(msg),
            TaskEvent::DrainDone { started } => self.on_drain_done(started),
            TaskEvent::SttConnected(Ok(link)) => self.on_stt_open(link),
            TaskEvent::SttConnected(Err(ConnectError::Failed(f))) => {
                self.fail(stt_failure_to_app_error(&f));
            }
            TaskEvent::SttConnected(Err(ConnectError::TimedOut)) => {
                self.fail(AppError::new(ErrorCode::SttConnect, copy::STT_CONNECT));
            }
            TaskEvent::Stt(ev) => self.on_stt_event(ev),
            TaskEvent::SttEnded => {
                if !self.stt_done() {
                    self.fail(AppError::new(ErrorCode::SttError, STT_CLOSED_EARLY));
                }
            }
            TaskEvent::SttWriteFailed(f) => {
                if !self.stt_done() {
                    self.fail(stt_failure_to_app_error(&f));
                }
            }
            TaskEvent::Delta { attempt, text } => self.on_delta(attempt, text),
            TaskEvent::StreamDone { attempt, result } => self.on_stream_done(attempt, result),
        }
    }

    fn stt_done(&self) -> bool {
        !self
            .live
            .as_ref()
            .is_some_and(|l| !matches!(l.stt, SttState::Done))
    }

    fn on_audio_started(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.stage != Stage::Starting {
            // Stop already accepted: no cap, no countdown.
            return;
        }
        let cap = self.timeouts.record_cap;
        let cap_ms = u32::try_from(cap.as_millis()).unwrap_or(u32::MAX);
        let deadline_ms = self.clock.epoch_ms().saturating_add(u64::from(cap_ms));
        live.stage = Stage::Recording;
        live.recording = Some(RecordingInfo {
            deadline_ms,
            cap_ms,
        });
        live.cap_deadline = Some(Instant::now() + cap);
        let id = live.id.clone();
        self.emit(CoreEvent::SessionRecording {
            session_id: id,
            deadline_ms,
            cap_ms,
        });
    }

    fn on_audio(&mut self, msg: AudioMsg) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match msg {
            AudioMsg::Frame(frame) => {
                if !live.capture_open {
                    // After the capture cutoff: nothing may follow CloseStream.
                    return;
                }
                let level = (!live.stopping()).then(|| CoreEvent::AudioLevel {
                    session_id: live.id.clone(),
                    rms: frame.rms,
                });
                match &live.stt {
                    SttState::Open { writer } => {
                        let _ = writer.send(WriterCmd::Frame(frame));
                    }
                    SttState::Connecting => {
                        if live.pre_buffer.len() >= PRE_CONNECT_BUFFER_FRAMES {
                            live.pre_buffer.pop_front();
                        }
                        live.pre_buffer.push_back(frame);
                    }
                    SttState::Done => {}
                }
                if let Some(level) = level {
                    self.emit(level);
                }
            }
            AudioMsg::DeviceLost { .. } => {
                if live.stopping() {
                    return;
                }
                let id = live.id.clone();
                self.emit(CoreEvent::AudioDevice {
                    session_id: id,
                    kind: DeviceNoticeKind::Lost,
                    message: copy::DEVICE_LOST.to_string(),
                });
                // Answer with what was captured.
                self.begin_stop();
            }
            AudioMsg::DeviceChanged { .. } => {
                if live.stopping() {
                    return;
                }
                let id = live.id.clone();
                self.emit(CoreEvent::AudioDevice {
                    session_id: id,
                    kind: DeviceNoticeKind::Changed,
                    message: copy::DEVICE_CHANGED.to_string(),
                });
            }
        }
    }

    fn on_stt_open(&mut self, link: SttLink) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if !matches!(live.stt, SttState::Connecting) {
            return;
        }
        let SttLink { sender, events } = link;
        let (writer_tx, writer_rx) = mpsc::unbounded_channel();
        // Flush the pre-connect buffer first, in capture order.
        for frame in live.pre_buffer.drain(..) {
            let _ = writer_tx.send(WriterCmd::Frame(frame));
        }
        let gen = live.gen;
        live.tasks.push(
            TaskKind::Writer,
            tokio::spawn(run_writer(gen, sender, writer_rx, self.internal_tx.clone())),
        );
        live.tasks.push(
            TaskKind::Reader,
            tokio::spawn(run_reader(gen, events, self.internal_tx.clone())),
        );
        live.stt = SttState::Open { writer: writer_tx };
        self.maybe_drain();
    }

    fn on_drain_done(&mut self, started: Instant) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if !live.capture_open {
            return;
        }
        let now = Instant::now();
        live.capture_open = false;
        live.drain_started = Some(started.min(now));
        live.drain_done = Some(now);
        if let SttState::Open { writer } = &live.stt {
            // Queued behind every frame: CloseStream is always last.
            let _ = writer.send(WriterCmd::Close);
            live.close_sent = true;
            live.finalize_deadline = Some(now + self.timeouts.stt_finalize);
        }
    }

    fn on_stt_event(&mut self, event: SttEvent) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if matches!(live.stt, SttState::Done) {
            return;
        }
        match event {
            SttEvent::Transcript { text, is_final } => {
                live.last_partial.clone_from(&text);
                let id = live.id.clone();
                self.emit(CoreEvent::SttPartial {
                    session_id: id,
                    text,
                    is_final,
                });
            }
            SttEvent::Flushed { transcript } => {
                if !live.close_sent {
                    self.fail(AppError::new(ErrorCode::SttError, STT_CLOSED_EARLY));
                    return;
                }
                live.flushed_at = Some(Instant::now());
                live.finalize_deadline = None;
                live.stt = SttState::Done;
                // The STT stream's job is done (invariant 5).
                live.tasks.abort_kind(TaskKind::Writer);
                live.tasks.abort_kind(TaskKind::Reader);
                if transcript.trim().is_empty() {
                    // Never call the LLM on an empty prompt (invariant 7).
                    self.fail(AppError::new(ErrorCode::NoSpeech, copy::NO_SPEECH));
                    return;
                }
                if transcript != live.last_partial {
                    let id = live.id.clone();
                    self.emit(CoreEvent::SttPartial {
                        session_id: id,
                        text: transcript.clone(),
                        is_final: true,
                    });
                }
                self.start_answer(transcript);
            }
            SttEvent::Failed(f) => self.fail(stt_failure_to_app_error(&f)),
        }
    }

    // ───────────────────────────── answer ─────────────────────────────

    fn start_answer(&mut self, transcript: String) {
        // Profile / call type / style are re-read here (memory only), so a
        // style chip flipped mid-recording applies; the provider and its key
        // stay the ones read at start.
        let config = self.settings.answer_config();
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let built = catch_unwind(AssertUnwindSafe(|| {
            let input = PromptInput {
                call_type: config.call_type,
                style: config.style,
                resume: &config.profile.resume,
                job_description: &config.profile.job_description,
                focus: &config.profile.focus,
                notes: &config.profile.notes,
            };
            let parts = build_prompt(&input, &transcript);
            live.provider.build_request(&parts, &live.llm_key)
        }));
        let Ok(request) = built else {
            self.fail(AppError::internal(BUILD_FAILED));
            return;
        };
        let now = Instant::now();
        live.stage = Stage::Answering;
        live.transcript = transcript;
        live.call_type = config.call_type;
        live.request = Some(Arc::new(request));
        live.first_token_deadline = Some(now + self.timeouts.llm_first_token);
        live.total_deadline = Some(now + self.timeouts.llm_total);
        self.spawn_attempt(1);
    }

    fn spawn_attempt(&mut self, attempt: u32) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let Some(request) = live.request.clone() else {
            return;
        };
        live.attempt = attempt;
        live.tasks.abort_kind(TaskKind::Stream);
        live.tasks.push(
            TaskKind::Stream,
            tokio::spawn(run_stream(
                live.gen,
                attempt,
                live.provider.clone(),
                request,
                self.internal_tx.clone(),
            )),
        );
    }

    fn on_delta(&mut self, attempt: u32, text: String) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.stage != Stage::Answering || attempt != live.attempt || text.is_empty() {
            return;
        }
        live.deltas += 1;
        if live.first_delta_at.is_none() {
            live.first_delta_at = Some(Instant::now());
            live.first_token_deadline = None;
        }
        let id = live.id.clone();
        self.emit(CoreEvent::LlmDelta {
            session_id: id,
            delta: text,
        });
    }

    fn on_stream_done(
        &mut self,
        attempt: u32,
        result: Result<
            callcore_contract::ports::StreamOutcome,
            callcore_contract::ports::ProviderFailure,
        >,
    ) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.stage != Stage::Answering || attempt != live.attempt {
            return;
        }
        match result {
            Ok(outcome) => {
                if outcome.answer.trim().is_empty() {
                    self.fail(AppError::new(ErrorCode::LlmHttp, EMPTY_ANSWER));
                    return;
                }
                let now = Instant::now();
                let base = live.accepted_at.unwrap_or(now);
                let total_ms = ms_between(base, now);
                let metrics = Metrics {
                    audio_drain_ms: match (live.drain_started, live.drain_done) {
                        (Some(a), Some(b)) => ms_between(a, b),
                        _ => 0,
                    },
                    stt_finalize_ms: match (live.drain_done, live.flushed_at) {
                        (Some(a), Some(b)) => ms_between(a, b),
                        _ => 0,
                    },
                    first_token_ms: live
                        .first_delta_at
                        .map_or(total_ms, |t| ms_between(base, t)),
                    total_ms,
                };
                let event = CoreEvent::LlmDone {
                    session_id: live.id.clone(),
                    transcript: std::mem::take(&mut live.transcript),
                    answer: outcome.answer,
                    finish: outcome.finish,
                    call_type: live.call_type,
                    metrics,
                };
                self.terminate(event);
            }
            Err(failure) => {
                // Retry ONCE, only on a connection-level failure before any
                // delta, with the SAME prepared request (invariant 6, §7).
                if failure.kind == callcore_contract::ports::ProviderFailureKind::Connect
                    && live.deltas == 0
                    && attempt == 1
                {
                    tracing::debug!(session = %live.id, "retrying provider connect");
                    self.spawn_attempt(2);
                    return;
                }
                self.fail(provider_failure_to_app_error(&failure));
            }
        }
    }

    // ───────────────────────────── deadlines ─────────────────────────────

    fn on_deadline(&mut self) {
        let now = Instant::now();
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let due = |d: Option<Instant>| d.is_some_and(|d| d <= now);
        if due(live.cap_deadline) {
            live.cap_deadline = None;
            if !live.stopping() {
                let id = live.id.clone();
                self.emit(CoreEvent::SessionAutostopped { session_id: id });
                self.begin_stop();
            }
        } else if due(live.finalize_deadline) {
            self.fail(AppError::new(ErrorCode::SttTimeout, copy::STT_TIMEOUT));
        } else if due(live.first_token_deadline) {
            self.fail(AppError::new(
                ErrorCode::LlmFirstTokenTimeout,
                copy::FIRST_TOKEN_TIMEOUT,
            ));
        } else if due(live.total_deadline) {
            self.fail(AppError::new(ErrorCode::LlmTimeout, copy::TOTAL_TIMEOUT));
        }
    }
}

//! Per-session child tasks and the audio lane. Every task reports back to the
//! actor with a [`Tagged`] message carrying its session generation; the actor
//! drops anything whose generation is not the live one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use callcore_contract::ports::{
    AnswerProvider, AudioError, AudioFrame, AudioMsg, AudioSource, DrainReport, FrameSink,
    PreparedRequest, ProviderFailure, StreamOutcome, SttConnector, SttEvent, SttFailure, SttSender,
};
use callcore_contract::Secret;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

/// Bound on `AudioSource::start` so a wedged device call can never freeze the
/// lane (and with it every later session).
pub(crate) const AUDIO_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on `AudioSource::stop_discard`.
pub(crate) const AUDIO_DISCARD_TIMEOUT: Duration = Duration::from_secs(3);
/// Extra slack above the drain bound the audio source applies itself.
pub(crate) const AUDIO_DRAIN_SLACK: Duration = Duration::from_millis(500);

/// A message from a child task, tagged with the generation (= session number)
/// of the session that spawned it.
pub(crate) struct Tagged {
    pub gen: u64,
    pub event: TaskEvent,
}

pub(crate) type InternalTx = mpsc::UnboundedSender<Tagged>;

pub(crate) enum TaskEvent {
    AudioStarted(Result<(), AudioError>),
    Audio(AudioMsg),
    /// Every frame the drain pushed has been relayed before this message.
    DrainDone {
        started: Instant,
    },
    SttConnected(Result<SttLink, ConnectError>),
    Stt(SttEvent),
    /// The STT event stream ended.
    SttEnded,
    SttWriteFailed(SttFailure),
    Delta {
        attempt: u32,
        text: String,
    },
    StreamDone {
        attempt: u32,
        result: Result<StreamOutcome, ProviderFailure>,
    },
}

pub(crate) enum ConnectError {
    Failed(SttFailure),
    TimedOut,
}

pub(crate) struct SttLink {
    pub sender: GuardedSender,
    pub events: mpsc::Receiver<SttEvent>,
}

/// Owns the STT sender and calls `abort()` when dropped, so every exit path
/// (task abort, stale connect result, teardown) drops the socket.
pub(crate) struct GuardedSender(Box<dyn SttSender>);

impl GuardedSender {
    pub fn new(sender: Box<dyn SttSender>) -> Self {
        Self(sender)
    }
}

impl Drop for GuardedSender {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Aborts every held task when dropped (and on [`TaskSet::abort_all`]).
#[derive(Default)]
pub(crate) struct TaskSet {
    tasks: Vec<(TaskKind, JoinHandle<()>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKind {
    Connect,
    Forwarder,
    Writer,
    Reader,
    Stream,
}

impl TaskSet {
    pub fn push(&mut self, kind: TaskKind, handle: JoinHandle<()>) {
        self.tasks.push((kind, handle));
    }

    pub fn abort_kind(&mut self, kind: TaskKind) {
        self.tasks.retain(|(k, h)| {
            if *k == kind {
                h.abort();
                false
            } else {
                true
            }
        });
    }

    pub fn abort_all(&mut self) {
        for (_, h) in self.tasks.drain(..) {
            h.abort();
        }
    }
}

impl Drop for TaskSet {
    fn drop(&mut self) {
        self.abort_all();
    }
}

// ───────────────────────────── STT ─────────────────────────────

pub(crate) async fn run_connect(
    gen: u64,
    stt: Arc<dyn SttConnector>,
    key: Secret,
    limit: Duration,
    tx: InternalTx,
) {
    let result = match timeout(limit, stt.connect(&key)).await {
        Ok(Ok(conn)) => Ok(SttLink {
            sender: GuardedSender::new(conn.sender),
            events: conn.events,
        }),
        Ok(Err(failure)) => Err(ConnectError::Failed(failure)),
        Err(_) => Err(ConnectError::TimedOut),
    };
    drop(key);
    let _ = tx.send(Tagged {
        gen,
        event: TaskEvent::SttConnected(result),
    });
}

pub(crate) enum WriterCmd {
    Frame(AudioFrame),
    Close,
}

/// Sends frames in FIFO order; `Close` is queued behind every frame, so
/// CloseStream is always last. After CloseStream the socket is kept open
/// (the server still has to flush) until the actor aborts this task.
pub(crate) async fn run_writer(
    gen: u64,
    mut sender: GuardedSender,
    mut rx: mpsc::UnboundedReceiver<WriterCmd>,
    tx: InternalTx,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Frame(frame) => {
                if let Err(f) = sender.0.send_audio(&frame).await {
                    let _ = tx.send(Tagged {
                        gen,
                        event: TaskEvent::SttWriteFailed(f),
                    });
                    return;
                }
            }
            WriterCmd::Close => {
                if let Err(f) = sender.0.close_stream().await {
                    let _ = tx.send(Tagged {
                        gen,
                        event: TaskEvent::SttWriteFailed(f),
                    });
                    return;
                }
                // Hold the socket until the session is done with it.
                std::future::pending::<()>().await;
            }
        }
    }
}

pub(crate) async fn run_reader(gen: u64, mut events: mpsc::Receiver<SttEvent>, tx: InternalTx) {
    while let Some(event) = events.recv().await {
        if tx
            .send(Tagged {
                gen,
                event: TaskEvent::Stt(event),
            })
            .is_err()
        {
            return;
        }
    }
    let _ = tx.send(Tagged {
        gen,
        event: TaskEvent::SttEnded,
    });
}

// ───────────────────────────── audio ─────────────────────────────

/// Relays capture messages to the actor. When the lane reports the drain
/// complete, relays whatever the drain pushed, sends `DrainDone` and exits —
/// dropping the receiver, so nothing can be captured after the cutoff.
pub(crate) async fn run_forwarder(
    gen: u64,
    mut rx: mpsc::UnboundedReceiver<AudioMsg>,
    mut drain_rx: oneshot::Receiver<Instant>,
    tx: InternalTx,
) {
    let mut sink_open = true;
    let started = loop {
        tokio::select! {
            biased;
            msg = rx.recv(), if sink_open => match msg {
                Some(m) => {
                    if tx.send(Tagged { gen, event: TaskEvent::Audio(m) }).is_err() {
                        return;
                    }
                }
                // The source cleared the sink; keep waiting for the drain report.
                None => sink_open = false,
            },
            r = &mut drain_rx => match r {
                Ok(started) => break started,
                Err(_) => return,
            },
        }
    };
    while let Ok(m) = rx.try_recv() {
        let _ = tx.send(Tagged {
            gen,
            event: TaskEvent::Audio(m),
        });
    }
    drop(rx);
    let _ = tx.send(Tagged {
        gen,
        event: TaskEvent::DrainDone { started },
    });
}

pub(crate) enum AudioOp {
    Start {
        gen: u64,
        sink: FrameSink,
    },
    /// `done` receives the instant the drain actually began.
    Drain {
        gen: u64,
        done: oneshot::Sender<Instant>,
    },
    Discard {
        gen: u64,
    },
    Exit,
}

/// The audio lane: ONE task issues every audio call, in the order the actor
/// queued them, so a superseded session's discard always reaches the device
/// before the next session's start.
pub(crate) async fn run_audio_lane(
    audio: Arc<dyn AudioSource>,
    mut rx: mpsc::UnboundedReceiver<AudioOp>,
    tx: InternalTx,
    live_gen: Arc<AtomicU64>,
    drain_timeout: Duration,
) {
    // Generation whose capture is (or may be) running.
    let mut active: Option<u64> = None;
    while let Some(op) = rx.recv().await {
        match op {
            AudioOp::Start { gen, sink } => {
                if live_gen.load(Ordering::SeqCst) != gen {
                    // Superseded before the device was touched.
                    continue;
                }
                let result = match timeout(AUDIO_START_TIMEOUT, audio.start(sink)).await {
                    Ok(Ok(())) => {
                        active = Some(gen);
                        Ok(())
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => {
                        // It may still come up; make sure the teardown stops it.
                        active = Some(gen);
                        Err(AudioError::Other("audio start timed out".into()))
                    }
                };
                let _ = tx.send(Tagged {
                    gen,
                    event: TaskEvent::AudioStarted(result),
                });
            }
            AudioOp::Drain { gen, done } => {
                if live_gen.load(Ordering::SeqCst) != gen {
                    // Session already gone; its Discard follows.
                    continue;
                }
                let started = Instant::now();
                if active == Some(gen) {
                    active = None;
                    let ok = matches!(
                        timeout(
                            drain_timeout + AUDIO_DRAIN_SLACK,
                            audio.stop_and_drain(drain_timeout)
                        )
                        .await,
                        Ok(Ok(DrainReport { .. }))
                    );
                    if !ok {
                        // Make sure the device is stopped and the sink cleared.
                        let _ = timeout(AUDIO_DISCARD_TIMEOUT, audio.stop_discard()).await;
                    }
                }
                let _ = done.send(started);
            }
            AudioOp::Discard { gen } => {
                if active == Some(gen) {
                    active = None;
                    let _ = timeout(AUDIO_DISCARD_TIMEOUT, audio.stop_discard()).await;
                }
            }
            AudioOp::Exit => {
                if active.take().is_some() {
                    let _ = timeout(AUDIO_DISCARD_TIMEOUT, audio.stop_discard()).await;
                }
                break;
            }
        }
    }
}

// ───────────────────────────── answer ─────────────────────────────

/// One provider attempt. Deltas are relayed in order; every delta the
/// provider sent is relayed BEFORE `StreamDone`. Aborting this task drops the
/// provider future (that is how the watchdogs cancel a stream).
pub(crate) async fn run_stream(
    gen: u64,
    attempt: u32,
    provider: Arc<dyn AnswerProvider>,
    request: Arc<PreparedRequest>,
    tx: InternalTx,
) {
    let (dtx, mut drx) = mpsc::unbounded_channel::<String>();
    let result = {
        let fut = provider.stream(&request, dtx);
        tokio::pin!(fut);
        loop {
            tokio::select! {
                biased;
                Some(text) = drx.recv() => {
                    let _ = tx.send(Tagged { gen, event: TaskEvent::Delta { attempt, text } });
                }
                r = &mut fut => break r,
            }
        }
    };
    while let Ok(text) = drx.try_recv() {
        let _ = tx.send(Tagged {
            gen,
            event: TaskEvent::Delta { attempt, text },
        });
    }
    let _ = tx.send(Tagged {
        gen,
        event: TaskEvent::StreamDone { attempt, result },
    });
}

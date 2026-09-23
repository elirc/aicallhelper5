//! System-audio loopback capture. PUBLIC API PINNED.
//!
//! * [`dsp`] — pure DSP: downmix, anti-aliased 16 kHz resampler, framer, RMS.
//! * [`LoopbackSource`] — the production [`AudioSource`]: one dedicated
//!   "audio-worker" OS thread owns every device object; the async methods
//!   send it a command and await the reply.
//! * [`backend`] / [`fake`] — the device seam and a scriptable fake for tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use callcore_contract::ports::{AudioError, AudioSource, DrainReport, FrameSink};
use tokio::sync::oneshot;

#[doc(hidden)]
pub mod backend;
pub mod dsp;
#[doc(hidden)]
pub mod fake;
mod worker;

use worker::{Command, SharedSlot, Worker, WorkerMsg};

/// How often the worker checks whether the default render device changed.
pub const DEFAULT_DEVICE_POLL: Duration = Duration::from_secs(1);
/// Extra time the async side waits for the worker's drain reply beyond the
/// drain timeout before cutting the sink off itself (worker wedged in a
/// device call).
pub const DRAIN_REPLY_GRACE: Duration = Duration::from_secs(1);
/// Bound on `shutdown` waiting for the worker thread to exit.
pub const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// WASAPI loopback of the default render device, driven by one dedicated
/// audio worker thread. Cheap to share behind `Arc`.
pub struct LoopbackSource {
    tx: Sender<WorkerMsg>,
    slot: SharedSlot,
    thread: Mutex<Option<(JoinHandle<()>, mpsc::Receiver<()>)>>,
    shut_down: AtomicBool,
}

impl LoopbackSource {
    /// Spawns the audio worker thread. Does NOT open a device yet.
    pub fn new() -> Result<Self, AudioError> {
        Self::with_backend(backend::CpalBackend, DEFAULT_DEVICE_POLL)
    }

    /// Same as [`LoopbackSource::new`] but over any capture backend (tests use
    /// [`fake::FakeBackend`]) and a custom default-device poll interval.
    #[doc(hidden)]
    pub fn with_backend<B>(backend: B, poll_interval: Duration) -> Result<Self, AudioError>
    where
        B: backend::CaptureBackend + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let slot: SharedSlot = Arc::default();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (worker_tx, worker_slot) = (tx.clone(), slot.clone());
        let handle = std::thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || {
                // Signals exit even if the worker panics.
                struct Done(mpsc::Sender<()>);
                impl Drop for Done {
                    fn drop(&mut self) {
                        let _ = self.0.send(());
                    }
                }
                let _done = Done(done_tx);
                // Built on this thread: it will own `!Send` device streams.
                Worker::new(backend, rx, worker_tx, worker_slot, poll_interval).run();
            })
            .map_err(|e| {
                tracing::error!(error = %e, "audio: could not spawn the worker thread");
                AudioError::Other("Could not start the audio worker.".into())
            })?;
        Ok(Self {
            tx,
            slot,
            thread: Mutex::new(Some((handle, done_rx))),
            shut_down: AtomicBool::new(false),
        })
    }

    /// Stop the worker thread (bounded; used at exit).
    pub fn shutdown(&self) {
        self.shut_down.store(true, Ordering::SeqCst);
        let _ = self.tx.send(WorkerMsg::Cmd(Command::Shutdown));
        let taken = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some((handle, done)) = taken {
            match done.recv_timeout(SHUTDOWN_JOIN_TIMEOUT) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = handle.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    tracing::warn!("audio: worker did not exit in time; detaching");
                }
            }
        }
        worker::clear_slot(&self.slot);
    }

    fn send(&self, cmd: Command) -> Result<(), AudioError> {
        if self.shut_down.load(Ordering::SeqCst) {
            return Err(AudioError::WorkerGone);
        }
        self.tx
            .send(WorkerMsg::Cmd(cmd))
            .map_err(|_| AudioError::WorkerGone)
    }
}

impl Drop for LoopbackSource {
    fn drop(&mut self) {
        // Non-blocking: ask the worker to stop; it drops any open stream.
        let _ = self.tx.send(WorkerMsg::Cmd(Command::Shutdown));
    }
}

#[async_trait]
impl AudioSource for LoopbackSource {
    async fn start(&self, sink: FrameSink) -> Result<(), AudioError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Start { sink, reply })?;
        rx.await.map_err(|_| AudioError::WorkerGone)?
    }

    async fn stop_discard(&self) {
        let (reply, rx) = oneshot::channel();
        match self.send(Command::StopDiscard { reply }) {
            Ok(()) => {
                // The worker clears the sink before replying. Do not clear
                // here: a concurrent start may already have installed a new one.
                let _ = rx.await;
            }
            Err(_) => worker::clear_slot(&self.slot),
        }
    }

    async fn stop_and_drain(&self, timeout: Duration) -> Result<DrainReport, AudioError> {
        let (reply, rx) = oneshot::channel();
        let epoch = worker::slot_epoch(&self.slot);
        self.send(Command::StopAndDrain { timeout, reply })?;
        match tokio::time::timeout(timeout + DRAIN_REPLY_GRACE, rx).await {
            Ok(Ok(report)) => Ok(report),
            Ok(Err(_)) => {
                worker::clear_slot(&self.slot);
                Err(AudioError::WorkerGone)
            }
            Err(_) => {
                // Worker wedged in a device call: enforce the capture cutoff
                // from here so nothing can follow the caller's CloseStream.
                tracing::warn!("audio: drain reply timed out; cutting the sink off");
                worker::clear_slot_if(&self.slot, epoch);
                Ok(DrainReport {
                    frames_flushed: 0,
                    timed_out: true,
                })
            }
        }
    }
}

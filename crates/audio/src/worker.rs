//! The audio worker: ONE OS thread that owns every device object (capture
//! streams need not be `Send`; they are built and dropped here) and all DSP
//! state.
//!
//! Everything reaches the worker through one FIFO channel of [`WorkerMsg`]:
//! commands from [`crate::LoopbackSource`], captured chunks and error reports
//! from the device callbacks. One queue means one explicit order: a chunk the
//! callback delivered before a stop command is always seen before (or while
//! draining for) that stop.
//!
//! Locking (spec §14.4): the device callback takes NO lock — it only converts
//! the chunk and sends it down the channel. The only lock here is the sink
//! slot, held briefly while pushing a message; no device call (open, play,
//! drop) ever happens while it is held.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use callcore_contract::copy;
use callcore_contract::ports::{AudioError, AudioFrame, AudioMsg, DrainReport, FrameSink};
use tokio::sync::oneshot;

use crate::backend::{CaptureBackend, CaptureError, CaptureStream, RawChunk};
use crate::dsp::{downmix_into, Pipeline};

pub(crate) enum Command {
    Start {
        sink: FrameSink,
        reply: oneshot::Sender<Result<(), AudioError>>,
    },
    StopDiscard {
        reply: oneshot::Sender<()>,
    },
    StopAndDrain {
        timeout: Duration,
        reply: oneshot::Sender<DrainReport>,
    },
    Shutdown,
}

pub(crate) enum WorkerMsg {
    Cmd(Command),
    /// Mono f32 at the stream's device rate, tagged with the stream generation.
    Data {
        gen: u64,
        samples: Vec<f32>,
    },
    StreamError {
        gen: u64,
        err: CaptureError,
    },
}

/// Where frames go. Shared with `LoopbackSource` only so an async-side drain
/// timeout can cut the sink off even while the worker is stuck in a device
/// call. Never locked by the device callback.
#[derive(Default)]
pub(crate) struct SinkSlot {
    sink: Option<FrameSink>,
    /// Bumped every time a sink is installed.
    epoch: u64,
}

pub(crate) type SharedSlot = Arc<Mutex<SinkSlot>>;

pub(crate) fn clear_slot(slot: &SharedSlot) {
    let taken = lock(slot).sink.take();
    drop(taken);
}

/// Epoch of the currently installed sink.
pub(crate) fn slot_epoch(slot: &SharedSlot) -> u64 {
    lock(slot).epoch
}

/// Clear the sink only if it is still the one installed at `epoch`.
pub(crate) fn clear_slot_if(slot: &SharedSlot, epoch: u64) {
    let mut g = lock(slot);
    if g.epoch == epoch {
        let taken = g.sink.take();
        drop(g);
        drop(taken);
    }
}

fn install_sink(slot: &SharedSlot, sink: FrameSink) {
    let mut g = lock(slot);
    g.epoch += 1;
    g.sink = Some(sink);
}

fn lock(slot: &SharedSlot) -> std::sync::MutexGuard<'_, SinkSlot> {
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

struct Capture {
    gen: u64,
    /// `None` once the device is lost.
    stream: Option<Box<dyn CaptureStream>>,
    device_id: String,
    dsp: Pipeline,
    next_poll: Instant,
}

pub(crate) struct Worker<B: CaptureBackend> {
    backend: B,
    rx: Receiver<WorkerMsg>,
    tx: Sender<WorkerMsg>,
    slot: SharedSlot,
    poll_interval: Duration,
    capture: Option<Capture>,
    /// Stream generation counter; every opened stream gets a fresh value and
    /// chunks tagged with any other value are dropped.
    next_gen: u64,
    /// Messages that arrived while a drain/rebuild was emptying the queue.
    deferred: VecDeque<WorkerMsg>,
}

/// Drop a stream without letting a panic in the backend's drop (it joins
/// the backend's capture thread) take down the worker.
fn drop_stream(stream: Box<dyn CaptureStream>) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(stream))).is_err() {
        tracing::error!("audio: panic while stopping the capture stream");
    }
}

impl<B: CaptureBackend> Worker<B> {
    pub(crate) fn new(
        backend: B,
        rx: Receiver<WorkerMsg>,
        tx: Sender<WorkerMsg>,
        slot: SharedSlot,
        poll_interval: Duration,
    ) -> Self {
        Self {
            backend,
            rx,
            tx,
            slot,
            poll_interval,
            capture: None,
            next_gen: 0,
            deferred: VecDeque::new(),
        }
    }

    pub(crate) fn run(mut self) {
        loop {
            let msg = if let Some(m) = self.deferred.pop_front() {
                m
            } else {
                let polling = self
                    .capture
                    .as_ref()
                    .filter(|c| c.stream.is_some())
                    .map(|c| c.next_poll);
                match polling {
                    Some(at) => {
                        let wait = at.saturating_duration_since(Instant::now());
                        match self.rx.recv_timeout(wait) {
                            Ok(m) => m,
                            Err(RecvTimeoutError::Timeout) => {
                                self.maybe_poll();
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    None => match self.rx.recv() {
                        Ok(m) => m,
                        Err(_) => break,
                    },
                }
            };
            match msg {
                WorkerMsg::Cmd(Command::Shutdown) => {
                    self.discard();
                    break;
                }
                WorkerMsg::Cmd(cmd) => self.handle_command(cmd),
                WorkerMsg::Data { gen, samples } => self.handle_data(gen, &samples),
                WorkerMsg::StreamError { gen, err } => self.handle_error(gen, err),
            }
            // Data keeps the queue busy, so recv_timeout alone would never
            // fire while capturing: check the poll deadline after every message.
            self.maybe_poll();
        }
        tracing::debug!("audio: worker exiting");
    }

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Start { sink, reply } => {
                let r = self.start(sink);
                let _ = reply.send(r);
            }
            Command::StopDiscard { reply } => {
                self.discard();
                let _ = reply.send(());
            }
            Command::StopAndDrain { timeout, reply } => {
                let report = self.drain(timeout);
                let _ = reply.send(report);
            }
            Command::Shutdown => unreachable!("handled in run"),
        }
    }

    fn push(&self, msg: AudioMsg) -> bool {
        let guard = lock(&self.slot);
        match &guard.sink {
            Some(s) => s.send(msg).is_ok(),
            None => false,
        }
    }

    fn push_frames(&self, frames: Vec<AudioFrame>) -> usize {
        let mut n = 0;
        for f in frames {
            if self.push(AudioMsg::Frame(f)) {
                n += 1;
            }
        }
        n
    }

    /// Open the default device on a fresh generation and start it playing.
    fn open_stream(&mut self) -> Result<(u64, Box<dyn CaptureStream>, u32, String), String> {
        self.next_gen += 1;
        let gen = self.next_gen;
        let tx_data = self.tx.clone();
        let on_data = Box::new(move |chunk: RawChunk<'_>| {
            // Minimal work on the device thread: downmix into an owned Vec and
            // hand it off. A panic (malformed chunk) drops just this chunk.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut mono =
                    Vec::with_capacity(chunk.samples.len() / chunk.channels.max(1) as usize);
                downmix_into(chunk.samples, chunk.channels, &mut mono);
                mono
            }));
            match r {
                Ok(samples) if !samples.is_empty() => {
                    let _ = tx_data.send(WorkerMsg::Data { gen, samples });
                }
                Ok(_) => {}
                Err(_) => tracing::warn!("audio: dropped a malformed capture chunk"),
            }
        });
        let tx_err = self.tx.clone();
        let on_error = Box::new(move |err: CaptureError| {
            let _ = tx_err.send(WorkerMsg::StreamError { gen, err });
        });
        let opened = self.backend.open(on_data, on_error)?;
        let mut stream = opened.stream;
        if opened.sample_rate == 0 {
            drop_stream(stream);
            return Err("device reported a 0 Hz sample rate".into());
        }
        if let Err(e) = stream.play() {
            drop_stream(stream);
            return Err(format!("play: {e}"));
        }
        Ok((gen, stream, opened.sample_rate, opened.device_id))
    }

    fn start(&mut self, sink: FrameSink) -> Result<(), AudioError> {
        // A start while capturing replaces the old capture (discard path).
        self.discard();
        match self.open_stream() {
            Ok((gen, stream, rate, device_id)) => {
                install_sink(&self.slot, sink);
                self.capture = Some(Capture {
                    gen,
                    stream: Some(stream),
                    device_id,
                    dsp: Pipeline::new(rate),
                    next_poll: Instant::now() + self.poll_interval,
                });
                tracing::info!(rate, "audio: capture started");
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, "audio: could not open the loopback device");
                Err(AudioError::DeviceOpen(copy::DEVICE_OPEN.into()))
            }
        }
    }

    fn handle_data(&mut self, gen: u64, samples: &[f32]) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        if cap.gen != gen || cap.stream.is_none() {
            return; // late chunk from a stopped / replaced / lost stream
        }
        let mut frames = Vec::new();
        cap.dsp.push(samples, &mut frames);
        self.push_frames(frames);
    }

    fn handle_error(&mut self, gen: u64, err: CaptureError) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        if cap.gen != gen || cap.stream.is_none() {
            return;
        }
        match err {
            CaptureError::Other(e) => tracing::warn!(error = %e, "audio: stream error"),
            CaptureError::DeviceLost => {
                tracing::warn!("audio: capture device lost");
                self.lose_device();
            }
        }
    }

    /// Stop the stream, deliver everything captured so far (incl. the final
    /// partial frame), then push ONE DeviceLost. The capture (and its sink)
    /// stays installed until the session stops it; nothing more is produced.
    fn lose_device(&mut self) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        let Some(stream) = cap.stream.take() else {
            return;
        };
        let gen = cap.gen;
        drop_stream(stream);
        self.drain_queue(gen, None);
        let mut frames = Vec::new();
        if let Some(cap) = self.capture.as_mut() {
            cap.dsp.flush(&mut frames);
        }
        self.push_frames(frames);
        self.push(AudioMsg::DeviceLost {
            message: copy::DEVICE_LOST.into(),
        });
    }

    /// Process every queued chunk of `gen`; stash everything else in
    /// `deferred`. Returns (frames pushed, timed out).
    fn drain_queue(&mut self, gen: u64, deadline: Option<Instant>) -> (usize, bool) {
        let mut pushed = 0;
        let mut timed_out = false;
        loop {
            match self.rx.try_recv() {
                Ok(WorkerMsg::Data { gen: g, samples }) if g == gen => {
                    if timed_out {
                        continue;
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        timed_out = true;
                        continue;
                    }
                    let mut frames = Vec::new();
                    if let Some(cap) = self.capture.as_mut() {
                        cap.dsp.push(&samples, &mut frames);
                    }
                    pushed += self.push_frames(frames);
                }
                Ok(WorkerMsg::Data { .. }) => {} // stale generation
                Ok(WorkerMsg::StreamError { gen: g, .. }) if g == gen => {} // stream already stopped
                Ok(other) => self.deferred.push_back(other),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        (pushed, timed_out)
    }

    fn discard(&mut self) {
        if let Some(cap) = self.capture.take() {
            if let Some(stream) = cap.stream {
                drop_stream(stream);
            }
            tracing::info!("audio: capture stopped (discard)");
        }
        // Always clear, so a late callback can never reach a later session.
        clear_slot(&self.slot);
    }

    fn drain(&mut self, timeout: Duration) -> DrainReport {
        let deadline = Instant::now() + timeout;
        let Some(mut cap) = self.capture.take() else {
            clear_slot(&self.slot);
            return DrainReport::default();
        };
        // 1. Stop device input. After the drop returns no callback runs, so
        //    every pre-stop chunk is already in the queue.
        if let Some(stream) = cap.stream.take() {
            drop_stream(stream);
        }
        let gen = cap.gen;
        self.capture = Some(cap);
        // 2. Process every queued chunk of this capture.
        let (mut frames_flushed, mut timed_out) = self.drain_queue(gen, Some(deadline));
        // 3. Resampler tail + final partial frame.
        if !timed_out {
            if Instant::now() >= deadline {
                timed_out = true;
            } else {
                let mut frames = Vec::new();
                if let Some(cap) = self.capture.as_mut() {
                    cap.dsp.flush(&mut frames);
                }
                frames_flushed += self.push_frames(frames);
            }
        }
        // 4. Capture cutoff: nothing reaches this sink any more.
        self.capture = None;
        clear_slot(&self.slot);
        if timed_out {
            tracing::warn!(frames_flushed, "audio: drain hit its time bound");
        } else {
            tracing::info!(frames_flushed, "audio: capture stopped (drained)");
        }
        DrainReport {
            frames_flushed,
            timed_out,
        }
    }

    fn maybe_poll(&mut self) {
        let due = match &self.capture {
            Some(c) if c.stream.is_some() => Instant::now() >= c.next_poll,
            _ => false,
        };
        if !due {
            return;
        }
        let current = self.backend.default_device_id();
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        cap.next_poll = Instant::now() + self.poll_interval;
        match current {
            Some(id) if id != cap.device_id => {
                tracing::info!("audio: default output device changed; rebuilding capture");
                self.rebuild();
            }
            // Same device, or not queryable right now (loss is reported by
            // the stream's error callback).
            _ => {}
        }
    }

    /// Follow a default-device change: stop the old stream, process its
    /// queued audio, open the new default device and keep the DSP chain
    /// continuous (a different device rate flushes the old resampler tail
    /// into the same framer and starts a new resampler).
    fn rebuild(&mut self) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        let Some(old) = cap.stream.take() else { return };
        let old_gen = cap.gen;
        drop_stream(old);
        self.drain_queue(old_gen, None);
        match self.open_stream() {
            Ok((gen, stream, rate, device_id)) => {
                let mut frames = Vec::new();
                if let Some(cap) = self.capture.as_mut() {
                    cap.dsp.change_rate(rate, &mut frames);
                    cap.gen = gen;
                    cap.stream = Some(stream);
                    cap.device_id = device_id;
                    cap.next_poll = Instant::now() + self.poll_interval;
                }
                self.push_frames(frames);
                self.push(AudioMsg::DeviceChanged {
                    message: copy::DEVICE_CHANGED.into(),
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "audio: could not reopen the new default device");
                let mut frames = Vec::new();
                if let Some(cap) = self.capture.as_mut() {
                    cap.dsp.flush(&mut frames);
                }
                self.push_frames(frames);
                self.push(AudioMsg::DeviceLost {
                    message: copy::DEVICE_LOST.into(),
                });
            }
        }
    }
}

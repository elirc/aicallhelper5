//! A scriptable in-memory [`CaptureBackend`] for tests (this crate's and any
//! other crate's). No audio hardware is touched.
//!
//! ```ignore
//! let (backend, handle) = FakeBackend::new(48_000);
//! let src = LoopbackSource::with_backend(backend, Duration::from_millis(10))?;
//! src.start(sink).await?;
//! handle.feed(&[0.1; 480]);
//! ```

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::backend::{
    CaptureBackend, CaptureError, CaptureStream, DataCallback, ErrorCallback, OpenedStream,
    RawChunk,
};
use crate::dsp::Samples;

struct StreamRec {
    on_data: DataCallback,
    on_error: ErrorCallback,
    playing: bool,
    dropped: bool,
    rate: u32,
}

struct State {
    rate: u32,
    default_device: Option<String>,
    fail_open: VecDeque<String>,
    fail_play: VecDeque<String>,
    streams: Vec<StreamRec>,
    /// When set, the next stream drop blocks until the paired sender fires.
    block_next_drop: Option<mpsc::Receiver<()>>,
}

type Shared = Arc<Mutex<State>>;

fn lock(s: &Shared) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

/// The backend half (moved into the worker thread).
pub struct FakeBackend {
    state: Shared,
}

/// The test's remote control.
#[derive(Clone)]
pub struct FakeHandle {
    state: Shared,
}

/// Releases a drop blocked by [`FakeHandle::block_next_drop`].
pub struct DropRelease(mpsc::Sender<()>);

impl DropRelease {
    pub fn release(self) {
        let _ = self.0.send(());
    }
}

struct FakeStream {
    index: usize,
    state: Shared,
}

impl CaptureStream for FakeStream {
    fn play(&mut self) -> Result<(), String> {
        let mut st = lock(&self.state);
        if let Some(e) = st.fail_play.pop_front() {
            return Err(e);
        }
        st.streams[self.index].playing = true;
        Ok(())
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        let blocker = lock(&self.state).block_next_drop.take();
        if let Some(rx) = blocker {
            let _ = rx.recv();
        }
        let mut st = lock(&self.state);
        let rec = &mut st.streams[self.index];
        rec.playing = false;
        rec.dropped = true;
    }
}

impl FakeBackend {
    /// Device "fake-0" at `rate` Hz.
    pub fn new(rate: u32) -> (FakeBackend, FakeHandle) {
        let state = Arc::new(Mutex::new(State {
            rate,
            default_device: Some("fake-0".into()),
            fail_open: VecDeque::new(),
            fail_play: VecDeque::new(),
            streams: Vec::new(),
            block_next_drop: None,
        }));
        (
            FakeBackend {
                state: state.clone(),
            },
            FakeHandle { state },
        )
    }
}

impl CaptureBackend for FakeBackend {
    fn open(
        &mut self,
        on_data: DataCallback,
        on_error: ErrorCallback,
    ) -> Result<OpenedStream, String> {
        let mut st = lock(&self.state);
        if let Some(e) = st.fail_open.pop_front() {
            return Err(e);
        }
        let device_id = st
            .default_device
            .clone()
            .ok_or_else(|| "no default device".to_string())?;
        let rate = st.rate;
        st.streams.push(StreamRec {
            on_data,
            on_error,
            playing: false,
            dropped: false,
            rate,
        });
        let index = st.streams.len() - 1;
        Ok(OpenedStream {
            stream: Box::new(FakeStream {
                index,
                state: self.state.clone(),
            }),
            sample_rate: rate,
            device_id,
        })
    }

    fn default_device_id(&self) -> Option<String> {
        lock(&self.state).default_device.clone()
    }
}

impl FakeHandle {
    /// Feed mono f32 samples through the newest stream's data callback, like
    /// the device would. Returns false (and delivers nothing) when there is
    /// no live, playing stream.
    pub fn feed(&self, samples: &[f32]) -> bool {
        self.feed_raw(1, Samples::F32(samples))
    }

    /// Feed an interleaved chunk in any format through the newest live stream.
    pub fn feed_raw(&self, channels: u16, samples: Samples<'_>) -> bool {
        let mut st = lock(&self.state);
        match st.streams.last_mut() {
            Some(rec) if rec.playing && !rec.dropped => {
                (rec.on_data)(RawChunk { channels, samples });
                true
            }
            _ => false,
        }
    }

    /// Invoke stream `index`'s data callback even if that stream was stopped
    /// (simulates a callback racing the stop). Returns false if no such stream.
    pub fn feed_late(&self, index: usize, samples: &[f32]) -> bool {
        let mut st = lock(&self.state);
        match st.streams.get_mut(index) {
            Some(rec) => {
                (rec.on_data)(RawChunk {
                    channels: 1,
                    samples: Samples::F32(samples),
                });
                true
            }
            None => false,
        }
    }

    /// Report an error from the newest stream's error callback.
    pub fn raise_error(&self, err: CaptureError) -> bool {
        let mut st = lock(&self.state);
        match st.streams.last_mut() {
            Some(rec) => {
                (rec.on_error)(err);
                true
            }
            None => false,
        }
    }

    pub fn fail_next_open(&self, why: &str) {
        lock(&self.state).fail_open.push_back(why.into());
    }

    pub fn fail_next_play(&self, why: &str) {
        lock(&self.state).fail_play.push_back(why.into());
    }

    /// Sample rate of streams opened from now on.
    pub fn set_rate(&self, rate: u32) {
        lock(&self.state).rate = rate;
    }

    pub fn set_default_device(&self, id: Option<&str>) {
        lock(&self.state).default_device = id.map(str::to_string);
    }

    /// Make the next stream drop block until the returned handle is released.
    pub fn block_next_drop(&self) -> DropRelease {
        let (tx, rx) = mpsc::channel();
        lock(&self.state).block_next_drop = Some(rx);
        DropRelease(tx)
    }

    pub fn open_count(&self) -> usize {
        lock(&self.state).streams.len()
    }

    /// True when the newest stream exists, was started and not dropped.
    pub fn is_capturing(&self) -> bool {
        lock(&self.state)
            .streams
            .last()
            .is_some_and(|r| r.playing && !r.dropped)
    }

    pub fn is_dropped(&self, index: usize) -> bool {
        lock(&self.state)
            .streams
            .get(index)
            .is_some_and(|r| r.dropped)
    }

    pub fn stream_rate(&self, index: usize) -> Option<u32> {
        lock(&self.state).streams.get(index).map(|r| r.rate)
    }
}

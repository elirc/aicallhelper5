//! The device seam. The worker thread only ever talks to a [`CaptureBackend`];
//! production uses [`crate::wasapi::WasapiBackend`] (WASAPI loopback), tests use
//! [`crate::fake::FakeBackend`].
//!
//! Every backend object is created, used and dropped on the audio worker
//! thread, so nothing here needs to be `Send` except the backend value that
//! is moved into that thread and the callbacks handed to the device.

use crate::dsp::Samples;

/// One interleaved chunk from the device callback.
#[derive(Debug, Clone, Copy)]
pub struct RawChunk<'a> {
    pub channels: u16,
    pub samples: Samples<'a>,
}

/// Errors reported by the device's error callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The device disappeared (unplug, driver reset).
    DeviceLost,
    /// Anything else; logged, capture continues.
    Other(String),
}

/// Called on the device's own thread for every captured chunk. Must be cheap.
pub type DataCallback = Box<dyn FnMut(RawChunk<'_>) + Send + 'static>;
/// Called on the device's own thread when the stream reports an error.
pub type ErrorCallback = Box<dyn FnMut(CaptureError) + Send + 'static>;

/// A built (not yet playing) capture stream. Dropping it stops capture; after
/// the drop returns, no further callbacks run.
pub trait CaptureStream {
    fn play(&mut self) -> Result<(), String>;
}

pub struct OpenedStream {
    pub stream: Box<dyn CaptureStream>,
    /// Device sample rate (Hz) of the chunks the callback will deliver.
    pub sample_rate: u32,
    /// Stable id of the device that was opened (compared against
    /// [`CaptureBackend::default_device_id`] to detect default-device changes).
    pub device_id: String,
}

pub trait CaptureBackend {
    /// Open the CURRENT default render device for loopback capture. Error
    /// strings are for logs only (never shown to the user).
    fn open(
        &mut self,
        on_data: DataCallback,
        on_error: ErrorCallback,
    ) -> Result<OpenedStream, String>;

    /// Id of the current default render device, `None` if there is none or
    /// it cannot be queried.
    fn default_device_id(&self) -> Option<String>;
}

/// Stand-in on platforms without WASAPI: every open fails (logged; the
/// session shows the closed "device open" copy).
#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct UnsupportedBackend;

#[cfg(not(windows))]
impl CaptureBackend for UnsupportedBackend {
    fn open(&mut self, _: DataCallback, _: ErrorCallback) -> Result<OpenedStream, String> {
        Err("system-audio loopback capture is only implemented on Windows".into())
    }

    fn default_device_id(&self) -> Option<String> {
        None
    }
}

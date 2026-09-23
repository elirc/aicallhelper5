//! The device seam. The worker thread only ever talks to a [`CaptureBackend`];
//! production uses [`CpalBackend`] (WASAPI loopback), tests use
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

// ───────────────────────────── cpal / WASAPI ─────────────────────────────

/// WASAPI loopback through cpal 0.15: `build_input_stream` on the default
/// OUTPUT device makes cpal open it with `AUDCLNT_STREAMFLAGS_LOOPBACK`.
#[derive(Debug, Default)]
pub struct CpalBackend;

struct CpalStream(cpal::Stream);

impl CaptureStream for CpalStream {
    fn play(&mut self) -> Result<(), String> {
        use cpal::traits::StreamTrait;
        self.0.play().map_err(|e| e.to_string())
    }
}

impl CaptureBackend for CpalBackend {
    fn open(
        &mut self,
        mut on_data: DataCallback,
        mut on_error: ErrorCallback,
    ) -> Result<OpenedStream, String> {
        use cpal::traits::{DeviceTrait, HostTrait};
        use cpal::SampleFormat;

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let device_id = device.name().map_err(|e| format!("device name: {e}"))?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("default output config: {e}"))?;
        let format = supported.sample_format();
        if !matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16 | SampleFormat::I32
        ) {
            return Err(format!("unsupported sample format {format:?}"));
        }
        let config: cpal::StreamConfig = supported.config();
        let channels = config.channels;
        let sample_rate = config.sample_rate.0;

        let data_cb = move |data: &cpal::Data, _: &cpal::InputCallbackInfo| {
            // Guard the whole body: a panic drops this chunk, never the stream.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let samples = match format {
                    SampleFormat::F32 => data.as_slice::<f32>().map(Samples::F32),
                    SampleFormat::I16 => data.as_slice::<i16>().map(Samples::I16),
                    SampleFormat::U16 => data.as_slice::<u16>().map(Samples::U16),
                    SampleFormat::I32 => data.as_slice::<i32>().map(Samples::I32),
                    _ => None,
                };
                if let Some(samples) = samples {
                    on_data(RawChunk { channels, samples });
                }
            }));
        };
        let err_cb = move |e: cpal::StreamError| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match e {
                cpal::StreamError::DeviceNotAvailable => on_error(CaptureError::DeviceLost),
                cpal::StreamError::BackendSpecific { err } => {
                    on_error(CaptureError::Other(err.to_string()))
                }
            }));
        };
        let stream = device
            .build_input_stream_raw(&config, format, data_cb, err_cb, None)
            .map_err(|e| format!("build loopback stream: {e}"))?;
        tracing::info!(
            sample_rate,
            channels,
            format = ?format,
            "audio: loopback stream built"
        );
        Ok(OpenedStream {
            stream: Box::new(CpalStream(stream)),
            sample_rate,
            device_id,
        })
    }

    fn default_device_id(&self) -> Option<String> {
        use cpal::traits::{DeviceTrait, HostTrait};
        cpal::default_host().default_output_device()?.name().ok()
    }
}

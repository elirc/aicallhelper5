//! Production [`CaptureBackend`]: WASAPI shared-mode loopback of the default
//! render device, straight on the `windows` crate.
//!
//! Why not cpal: cpal 0.15 opens loopback in event-driven mode, and on real
//! hardware that delivered ~0.13 s of audio out of 3 s (see
//! `docs/testing/audio.md`). Loopback clients cannot rely on the buffer event,
//! so this backend POLLS instead.
//!
//! Threading: every COM object of a stream lives on ONE dedicated
//! "audio-wasapi" thread (MTA). [`WasapiBackend::open`] spawns it and waits
//! for the device to be initialized; [`CaptureStream::play`] tells it to start
//! the client; dropping the stream disconnects its control channel and JOINS
//! the thread. Before exiting, the thread reads every packet still in the
//! WASAPI buffer, so once the drop returns every captured chunk has been
//! handed to `on_data` and no further callback can run (the worker's capture
//! cutoff relies on this).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{GUID, HRESULT};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

use crate::backend::{
    CaptureBackend, CaptureError, CaptureStream, DataCallback, ErrorCallback, OpenedStream,
    RawChunk,
};
use crate::dsp::Samples;
use crate::wasapi_format::{
    classify, convert_to_f32, GapFiller, RawFormat, SampleFormat, SubFormat, WAVE_FORMAT_EXTENSIBLE,
};

/// Shared-mode buffer requested from the engine (100 ns units): 200 ms gives
/// the 10 ms poll plenty of slack against scheduler hiccups.
const BUFFER_HNS: i64 = 2_000_000;
/// How often the capture thread drains the WASAPI buffer.
const POLL: Duration = Duration::from_millis(10);
/// Bound on `open` waiting for the capture thread to initialize the device.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on `play` waiting for `IAudioClient::Start`.
const PLAY_TIMEOUT: Duration = Duration::from_secs(5);
/// Consecutive non-fatal poll errors (~0.5 s) after which the device is
/// treated as lost instead of logging forever.
const MAX_CONSECUTIVE_ERRORS: u32 = 50;
/// Largest silence chunk handed to `on_data` at once (frames).
const MAX_SILENCE_CHUNK: usize = 4_800;

const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// WASAPI loopback of the default render device (polling, not event-driven).
#[derive(Debug, Default)]
pub struct WasapiBackend;

/// Balances a successful `CoInitializeEx` on drop (same thread).
struct ComGuard {
    owned: bool,
}

impl ComGuard {
    fn init_mta() -> Self {
        // SAFETY: plain COM init on the current thread; balanced in Drop only
        // when it succeeded (S_OK or S_FALSE). RPC_E_CHANGED_MODE (thread is
        // already STA) still leaves COM usable and must not be balanced.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        ComGuard { owned: hr.is_ok() }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

fn hr_text(e: &windows::core::Error) -> String {
    format!("{:#010x} {}", e.code().0, e.message())
}

fn is_device_lost(code: HRESULT) -> bool {
    code == AUDCLNT_E_DEVICE_INVALIDATED || code == AUDCLNT_E_SERVICE_NOT_RUNNING
}

fn default_render_device() -> Result<IMMDevice, String> {
    // SAFETY: COM is initialized on this thread by the caller.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("device enumerator: {}", hr_text(&e)))?;
        enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("default render endpoint: {}", hr_text(&e)))
    }
}

fn device_id(device: &IMMDevice) -> Result<String, String> {
    // SAFETY: GetId returns a CoTaskMem string that we free after copying.
    unsafe {
        let p = device
            .GetId()
            .map_err(|e| format!("device id: {}", hr_text(&e)))?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map_err(|e| format!("device id utf16: {e}"))
    }
}

struct MixFormat {
    rate: u32,
    channels: u16,
    block_align: usize,
    format: SampleFormat,
}

/// Read and classify the engine mix format. `p` must be a valid
/// `WAVEFORMATEX` from `GetMixFormat`.
unsafe fn read_mix_format(p: *const WAVEFORMATEX) -> Result<MixFormat, String> {
    // SAFETY (caller): `p` points to a valid WAVEFORMATEX; when the tag is
    // EXTENSIBLE and cbSize is large enough, it is a WAVEFORMATEXTENSIBLE.
    // The struct is packed, so read it unaligned.
    let wf = unsafe { std::ptr::read_unaligned(p) };
    let ext_size =
        (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()) as u16;
    let sub_format = if wf.wFormatTag == WAVE_FORMAT_EXTENSIBLE && wf.cbSize >= ext_size {
        let ext = unsafe { std::ptr::read_unaligned(p as *const WAVEFORMATEXTENSIBLE) };
        let sub = ext.SubFormat;
        Some(if sub == SUBTYPE_IEEE_FLOAT {
            SubFormat::Float
        } else if sub == SUBTYPE_PCM {
            SubFormat::Pcm
        } else {
            SubFormat::Other
        })
    } else {
        None
    };
    let raw = RawFormat {
        tag: wf.wFormatTag,
        channels: wf.nChannels,
        bits_per_sample: wf.wBitsPerSample,
        block_align: wf.nBlockAlign,
        sub_format,
    };
    let format = classify(raw)?;
    Ok(MixFormat {
        rate: wf.nSamplesPerSec,
        channels: wf.nChannels,
        block_align: usize::from(wf.nBlockAlign),
        format,
    })
}

/// Everything the capture thread owns once the device is initialized.
struct Device {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    mix: MixFormat,
}

fn open_device() -> Result<(Device, String), String> {
    let device = default_render_device()?;
    let id = device_id(&device)?;
    // SAFETY: COM calls on the thread that initialized COM; the mix format
    // pointer is freed with CoTaskMemFree on every path after Initialize.
    unsafe {
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("activate IAudioClient: {}", hr_text(&e)))?;
        let pwfx = client
            .GetMixFormat()
            .map_err(|e| format!("mix format: {}", hr_text(&e)))?;
        let mix = read_mix_format(pwfx);
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            BUFFER_HNS,
            0,
            pwfx,
            None,
        );
        CoTaskMemFree(Some(pwfx as *const _));
        let mix = mix?;
        init.map_err(|e| format!("initialize loopback: {}", hr_text(&e)))?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("capture client: {}", hr_text(&e)))?;
        Ok((
            Device {
                client,
                capture,
                mix,
            },
            id,
        ))
    }
}

enum Ctl {
    Play(Sender<Result<(), String>>),
}

pub struct WasapiStream {
    ctl: Option<Sender<Ctl>>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureStream for WasapiStream {
    fn play(&mut self) -> Result<(), String> {
        let ctl = self.ctl.as_ref().ok_or("stream already stopped")?;
        let (tx, rx) = mpsc::channel();
        ctl.send(Ctl::Play(tx))
            .map_err(|_| "capture thread exited".to_string())?;
        rx.recv_timeout(PLAY_TIMEOUT)
            .map_err(|_| "capture thread did not start in time".to_string())?
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        // Disconnecting the control channel is the stop signal.
        drop(self.ctl.take());
        if let Some(t) = self.thread.take() {
            if t.join().is_err() {
                tracing::error!("audio: the WASAPI capture thread panicked");
            }
        }
    }
}

impl CaptureBackend for WasapiBackend {
    fn open(
        &mut self,
        on_data: DataCallback,
        on_error: ErrorCallback,
    ) -> Result<OpenedStream, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, String), String>>();
        let (ctl_tx, ctl_rx) = mpsc::channel::<Ctl>();
        let thread = std::thread::Builder::new()
            .name("audio-wasapi".into())
            .spawn(move || capture_thread(ready_tx, ctl_rx, on_data, on_error))
            .map_err(|e| format!("spawn capture thread: {e}"))?;
        let mut stream = WasapiStream {
            ctl: Some(ctl_tx),
            thread: Some(thread),
        };
        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok((sample_rate, device_id))) => Ok(OpenedStream {
                stream: Box::new(stream),
                sample_rate,
                device_id,
            }),
            Ok(Err(e)) => Err(e), // thread already exited; drop joins it
            Err(_) => {
                // Wedged in a device call: detach rather than block the worker.
                // It exits on its own once the call returns (ctl is gone).
                drop(stream.ctl.take());
                drop(stream.thread.take());
                Err("capture thread did not initialize in time".into())
            }
        }
    }

    fn default_device_id(&self) -> Option<String> {
        let _com = ComGuard::init_mta();
        default_render_device()
            .and_then(|d| device_id(&d))
            .map_err(|e| tracing::debug!(error = %e, "audio: default device query failed"))
            .ok()
    }
}

/// Why the poll loop stopped.
enum Exit {
    Stopped,
    Lost,
}

fn capture_thread(
    ready: Sender<Result<(u32, String), String>>,
    ctl: Receiver<Ctl>,
    mut on_data: DataCallback,
    mut on_error: ErrorCallback,
) {
    let _com = ComGuard::init_mta();
    let dev = match open_device() {
        Ok((dev, id)) => {
            tracing::info!(
                rate = dev.mix.rate,
                channels = dev.mix.channels,
                format = ?dev.mix.format,
                "audio: WASAPI loopback initialized"
            );
            let _ = ready.send(Ok((dev.mix.rate, id)));
            dev
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // Wait for play (or a drop before play).
    let started = loop {
        match ctl.recv() {
            Ok(Ctl::Play(reply)) => {
                // SAFETY: COM call on the owning thread.
                match unsafe { dev.client.Start() } {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                        break true;
                    }
                    Err(e) => {
                        let _ = reply.send(Err(format!("start: {}", hr_text(&e))));
                    }
                }
            }
            Err(_) => break false,
        }
    };
    if !started {
        return;
    }
    let mut poller = Poller::new(&dev, &mut on_data);
    let exit = loop {
        match ctl.recv_timeout(POLL) {
            Ok(Ctl::Play(reply)) => {
                let _ = reply.send(Ok(()));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // Stop: hand over whatever is still buffered (and the
                // trailing silence of a gap), then exit.
                if poller.read_packets().is_ok() {
                    poller.finish_gap();
                }
                break Exit::Stopped;
            }
        }
        match poller.read_packets() {
            Ok(()) => {
                poller.errors = 0;
                poller.fill_gap();
            }
            Err(e) if is_device_lost(e.code()) => break Exit::Lost,
            Err(e) => {
                poller.errors += 1;
                if poller.errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::warn!(error = %hr_text(&e), "audio: persistent capture errors");
                    break Exit::Lost;
                }
                if poller.errors == 1 {
                    on_error(CaptureError::Other(hr_text(&e)));
                }
            }
        }
    };
    drop(poller);
    // SAFETY: COM call on the owning thread; failure (device gone) is fine.
    let _ = unsafe { dev.client.Stop() };
    if let Exit::Lost = exit {
        on_error(CaptureError::DeviceLost);
    }
}

struct Poller<'a> {
    dev: &'a Device,
    on_data: &'a mut DataCallback,
    t0: Instant,
    gap: GapFiller,
    scratch: Vec<f32>,
    zeros: Vec<f32>,
    errors: u32,
}

impl<'a> Poller<'a> {
    fn new(dev: &'a Device, on_data: &'a mut DataCallback) -> Self {
        Self {
            dev,
            on_data,
            t0: Instant::now(),
            gap: GapFiller::new(dev.mix.rate, Duration::ZERO),
            scratch: Vec::new(),
            zeros: Vec::new(),
            errors: 0,
        }
    }

    fn emit(&mut self, channels: u16, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let cb = &mut *self.on_data;
        // A panicking callback drops this chunk, never the capture thread.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cb(RawChunk {
                channels,
                samples: Samples::F32(samples),
            })
        }));
    }

    fn emit_silence(&mut self, frames: u64) {
        // Mono zeros: the worker downmixes per chunk, so channel count is free.
        let mut left = frames;
        while left > 0 {
            let n = left.min(MAX_SILENCE_CHUNK as u64) as usize;
            if self.zeros.len() < n {
                self.zeros.resize(n, 0.0);
            }
            let zeros = std::mem::take(&mut self.zeros);
            self.emit(1, &zeros[..n]);
            self.zeros = zeros;
            left -= n as u64;
        }
    }

    /// Drain every packet currently in the WASAPI buffer.
    fn read_packets(&mut self) -> windows::core::Result<()> {
        let mix_channels = self.dev.mix.channels;
        let block_align = self.dev.mix.block_align;
        let format = self.dev.mix.format;
        loop {
            // SAFETY: COM calls on the owning thread. The buffer returned by
            // GetBuffer is valid for `frames * block_align` bytes until the
            // matching ReleaseBuffer, and is only read in between.
            unsafe {
                if self.dev.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.dev
                    .capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0 {
                    tracing::trace!("audio: WASAPI data discontinuity");
                }
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                if frames > 0 && !silent && !data.is_null() {
                    let bytes = std::slice::from_raw_parts(data, frames as usize * block_align);
                    convert_to_f32(bytes, format, &mut self.scratch);
                }
                self.dev.capture.ReleaseBuffer(frames)?;
                let now = self.t0.elapsed();
                self.gap.on_packet(now);
                if silent || data.is_null() {
                    self.emit_silence(u64::from(frames));
                } else if frames > 0 {
                    let scratch = std::mem::take(&mut self.scratch);
                    self.emit(mix_channels, &scratch);
                    self.scratch = scratch;
                }
            }
        }
    }

    fn finish_gap(&mut self) {
        let due = self.gap.finish(self.t0.elapsed());
        if due > 0 {
            self.emit_silence(due);
        }
    }

    fn fill_gap(&mut self) {
        let due = self.gap.due(self.t0.elapsed());
        if due > 0 {
            self.emit_silence(due);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE_0;

    #[test]
    fn subtype_guids_match_ksmedia() {
        // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT from ksmedia.h.
        assert_eq!(
            format!("{SUBTYPE_PCM:?}"),
            "00000001-0000-0010-8000-00AA00389B71"
        );
        assert_eq!(
            format!("{SUBTYPE_IEEE_FLOAT:?}"),
            "00000003-0000-0010-8000-00AA00389B71"
        );
    }

    #[test]
    fn read_mix_format_parses_extensible_float() {
        let mut ext = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                nChannels: 2,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 384_000,
                nBlockAlign: 8,
                wBitsPerSample: 32,
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 32,
            },
            dwChannelMask: 3,
            SubFormat: SUBTYPE_IEEE_FLOAT,
        };
        // SAFETY: points at a live, fully initialized WAVEFORMATEXTENSIBLE.
        let mix = unsafe { read_mix_format(&ext as *const _ as *const WAVEFORMATEX) }.unwrap();
        assert_eq!(mix.rate, 48_000);
        assert_eq!(mix.channels, 2);
        assert_eq!(mix.block_align, 8);
        assert_eq!(mix.format, SampleFormat::F32);

        ext.SubFormat = SUBTYPE_PCM;
        ext.Format.wBitsPerSample = 24;
        ext.Format.nBlockAlign = 6;
        // SAFETY: as above.
        let mix = unsafe { read_mix_format(&ext as *const _ as *const WAVEFORMATEX) }.unwrap();
        assert_eq!(mix.format, SampleFormat::I24);
    }

    #[test]
    fn device_lost_codes() {
        assert!(is_device_lost(AUDCLNT_E_DEVICE_INVALIDATED));
        assert!(is_device_lost(AUDCLNT_E_SERVICE_NOT_RUNNING));
        assert!(!is_device_lost(HRESULT(0x8007_000E_u32 as i32)));
    }
}

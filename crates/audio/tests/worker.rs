//! Worker / LoopbackSource behaviour over the fake capture backend. No audio
//! hardware is touched; every wait is on a condition (bounded), never a fixed
//! sleep.

use std::time::{Duration, Instant};

use callcore_audio::backend::CaptureError;
use callcore_audio::dsp::{f32_to_i16, Resampler, Samples};
use callcore_audio::fake::{FakeBackend, FakeHandle};
use callcore_audio::LoopbackSource;
use callcore_contract::config::FRAME_SAMPLES;
use callcore_contract::copy;
use callcore_contract::ports::{AudioError, AudioMsg, AudioSource};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(5);
const NO_POLL: Duration = Duration::from_secs(3600);

fn source(rate: u32, poll: Duration) -> (LoopbackSource, FakeHandle) {
    let (backend, handle) = FakeBackend::new(rate);
    let src = LoopbackSource::with_backend(backend, poll).expect("worker");
    (src, handle)
}

fn ramp(n: usize, offset: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (((i + offset) % 2000) as f32 - 1000.0) / 1100.0)
        .collect()
}

async fn next(rx: &mut mpsc::UnboundedReceiver<AudioMsg>) -> Option<AudioMsg> {
    tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("timed out waiting for audio msg")
}

/// Everything until the sink is closed.
async fn collect(rx: &mut mpsc::UnboundedReceiver<AudioMsg>) -> Vec<AudioMsg> {
    let mut out = Vec::new();
    while let Some(m) = next(rx).await {
        out.push(m);
    }
    out
}

fn samples_of(msgs: &[AudioMsg]) -> Vec<i16> {
    msgs.iter()
        .flat_map(|m| match m {
            AudioMsg::Frame(f) => f.samples.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn frame_count(msgs: &[AudioMsg]) -> usize {
    msgs.iter()
        .filter(|m| matches!(m, AudioMsg::Frame(_)))
        .count()
}

async fn wait_until(mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < WAIT, "condition not reached");
        tokio::task::yield_now().await;
        std::thread::yield_now();
    }
}

#[tokio::test]
async fn start_then_frames_flow() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    let x = ramp(FRAME_SAMPLES * 2, 0);
    for c in x.chunks(160) {
        assert!(fake.feed(c));
    }
    for k in 0..2 {
        match next(&mut rx).await {
            Some(AudioMsg::Frame(f)) => {
                assert_eq!(f.samples.len(), FRAME_SAMPLES);
                let expect: Vec<i16> = x[k * FRAME_SAMPLES..(k + 1) * FRAME_SAMPLES]
                    .iter()
                    .map(|&v| f32_to_i16(v))
                    .collect();
                assert_eq!(f.samples, expect);
                assert!(f.rms > 0.0 && f.rms <= 1.0);
            }
            other => panic!("expected frame, got {other:?}"),
        }
    }
    src.stop_discard().await;
}

#[tokio::test]
async fn start_returns_only_once_stream_is_playing() {
    let (src, fake) = source(48_000, NO_POLL);
    let (tx, _rx) = mpsc::unbounded_channel();
    assert!(!fake.is_capturing());
    src.start(tx).await.unwrap();
    assert!(fake.is_capturing());
    assert_eq!(fake.open_count(), 1);
}

#[tokio::test]
async fn drain_delivers_every_pre_stop_sample_including_final_partial_frame() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    let x = ramp(5_000, 3);
    for c in x.chunks(160) {
        assert!(fake.feed(c));
    }
    let report = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert!(!report.timed_out);
    assert!(
        report.frames_flushed >= 1,
        "at least the final partial frame is flushed in the drain"
    );
    let msgs = collect(&mut rx).await; // ends: sink cleared
    let expect: Vec<i16> = x.iter().map(|&v| f32_to_i16(v)).collect();
    assert_eq!(samples_of(&msgs), expect);
    assert_eq!(frame_count(&msgs), 3);
    match msgs.last() {
        Some(AudioMsg::Frame(f)) => assert_eq!(f.samples.len(), 5_000 - 2 * FRAME_SAMPLES),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn drain_at_48k_delivers_exact_resampled_total_including_tail() {
    let (src, fake) = source(48_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    let x = ramp(48_017, 0);
    for c in x.chunks(441) {
        fake.feed(c);
    }
    let report = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert!(!report.timed_out);
    let msgs = collect(&mut rx).await;
    assert_eq!(
        samples_of(&msgs).len() as u64,
        Resampler::total_output_len(48_000, 48_017)
    );
    assert_eq!(samples_of(&msgs).len(), 16_006);
}

#[tokio::test]
async fn nothing_reaches_the_sink_after_drain_returns() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&ramp(1_000, 0));
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    // Device input stopped: the stream is dropped and a racing callback is ignored.
    assert!(fake.is_dropped(0));
    assert!(!fake.feed(&[0.5; 4_096]));
    fake.feed_late(0, &[0.5; 4_096]);
    let msgs = collect(&mut rx).await;
    assert_eq!(samples_of(&msgs).len(), 1_000);
}

#[tokio::test]
async fn stop_discard_pushes_nothing_more() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&ramp(1_000, 0)); // less than one frame: stays in the framer
    src.stop_discard().await;
    fake.feed_late(0, &[0.5; 4_096]);
    let msgs = collect(&mut rx).await;
    assert!(msgs.is_empty(), "{msgs:?}");
    assert!(fake.is_dropped(0));
}

#[tokio::test]
async fn late_callback_after_stop_never_reaches_next_session() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    src.start(tx1).await.unwrap();
    src.stop_discard().await;
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    src.start(tx2).await.unwrap();
    // Old stream's callback fires late with loud audio.
    fake.feed_late(0, &[0.9; 4_096]);
    // New stream delivers a distinct marker.
    fake.feed(&[0.25; 100]);
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    let msgs = collect(&mut rx2).await;
    assert_eq!(samples_of(&msgs), vec![f32_to_i16(0.25); 100]);
    assert!(collect(&mut rx1).await.is_empty());
}

#[tokio::test]
async fn callback_panic_drops_only_that_chunk() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&[0.1; 1_000]);
    // A zero-channel chunk makes the downmix panic inside the callback.
    assert!(fake.feed_raw(0, Samples::F32(&[0.7; 64])));
    assert!(fake.is_capturing(), "stream survives the panic");
    fake.feed(&[0.2; 1_000]);
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    let got = samples_of(&collect(&mut rx).await);
    let mut expect = vec![f32_to_i16(0.1); 1_000];
    expect.extend(vec![f32_to_i16(0.2); 1_000]);
    assert_eq!(got, expect);
}

#[tokio::test]
async fn open_failure_maps_to_device_open_copy_without_details() {
    let (src, fake) = source(48_000, NO_POLL);
    fake.fail_next_open("IAudioClient::Initialize hr=0x88890004 secret-detail");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let err = src.start(tx).await.unwrap_err();
    assert_eq!(err, AudioError::DeviceOpen(copy::DEVICE_OPEN.into()));
    assert!(!err.to_string().contains("0x8889"));
    assert!(
        collect(&mut rx).await.is_empty(),
        "sink not kept after a failed start"
    );
    // The worker is still usable.
    let (tx, _rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
}

#[tokio::test]
async fn play_failure_maps_to_device_open_and_drops_stream() {
    let (src, fake) = source(48_000, NO_POLL);
    fake.fail_next_play("play refused");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let err = src.start(tx).await.unwrap_err();
    assert_eq!(err, AudioError::DeviceOpen(copy::DEVICE_OPEN.into()));
    assert!(fake.is_dropped(0));
    assert!(collect(&mut rx).await.is_empty());
}

#[tokio::test]
async fn drain_time_bound_exceeded_on_worker_reports_timed_out_and_clears_sink() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&[0.1; 1_000]);
    let report = src.stop_and_drain(Duration::ZERO).await.unwrap();
    assert!(report.timed_out);
    // Sink cleared: the channel closes.
    let msgs = collect(&mut rx).await;
    assert!(samples_of(&msgs).len() <= 1_000);
}

#[tokio::test]
async fn drain_reply_timeout_when_worker_is_wedged_still_cuts_the_sink() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&[0.1; 1_000]);
    let release = fake.block_next_drop(); // the device stop hangs
    tokio::time::pause();
    let report = src
        .stop_and_drain(Duration::from_millis(100))
        .await
        .unwrap();
    assert!(report.timed_out);
    // The sink is closed although the worker is still stuck in the stop.
    let _ = collect(&mut rx).await;
    release.release();
    // Back to real time: a paused clock would auto-advance past the next
    // drain's reply timeout while the worker thread is busy.
    tokio::time::resume();
    // The worker recovers and serves the next session.
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    src.start(tx2).await.unwrap();
    fake.feed(&[0.3; 10]);
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(
        samples_of(&collect(&mut rx2).await),
        vec![f32_to_i16(0.3); 10]
    );
}

#[tokio::test]
async fn device_lost_flushes_captured_audio_then_reports_once() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&[0.1; 1_000]);
    fake.raise_error(CaptureError::DeviceLost);
    fake.raise_error(CaptureError::DeviceLost);
    match next(&mut rx).await {
        Some(AudioMsg::Frame(f)) => assert_eq!(f.samples.len(), 1_000),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        next(&mut rx).await,
        Some(AudioMsg::DeviceLost {
            message: copy::DEVICE_LOST.into()
        })
    );
    assert!(fake.is_dropped(0), "stream stopped after loss");
    fake.feed_late(0, &[0.5; 4_096]);
    let report = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.frames_flushed, 0);
    assert!(!report.timed_out);
    let rest = collect(&mut rx).await;
    assert!(
        rest.is_empty(),
        "no second DeviceLost and no frames after loss: {rest:?}"
    );
}

#[tokio::test]
async fn non_fatal_stream_error_keeps_capturing() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.raise_error(CaptureError::Other("glitch".into()));
    fake.feed(&[0.1; 500]);
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    let msgs = collect(&mut rx).await;
    assert_eq!(samples_of(&msgs).len(), 500);
    assert!(msgs.iter().all(|m| matches!(m, AudioMsg::Frame(_))));
}

#[tokio::test]
async fn default_device_change_rebuilds_stream_and_keeps_continuity() {
    let (src, fake) = source(48_000, Duration::from_millis(10));
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&ramp(4_800, 0));
    fake.set_rate(44_100);
    fake.set_default_device(Some("fake-1"));
    // Wait for the switch notice.
    let mut before = Vec::new();
    loop {
        match next(&mut rx).await {
            Some(AudioMsg::DeviceChanged { message }) => {
                assert_eq!(message, copy::DEVICE_CHANGED);
                break;
            }
            Some(m) => before.push(m),
            None => panic!("sink closed"),
        }
    }
    assert_eq!(fake.open_count(), 2);
    assert!(fake.is_dropped(0));
    assert!(fake.is_capturing());
    assert_eq!(fake.stream_rate(1), Some(44_100));
    fake.feed(&ramp(4_410, 0));
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    let after = collect(&mut rx).await;
    assert!(after.iter().all(|m| matches!(m, AudioMsg::Frame(_))));
    let total = samples_of(&before).len() + samples_of(&after).len();
    let expect =
        Resampler::total_output_len(48_000, 4_800) + Resampler::total_output_len(44_100, 4_410);
    assert_eq!(total as u64, expect);
}

#[tokio::test]
async fn default_device_change_with_failed_reopen_reports_device_lost() {
    let (src, fake) = source(16_000, Duration::from_millis(10));
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    fake.feed(&[0.1; 300]);
    fake.fail_next_open("new device refused");
    fake.set_default_device(Some("fake-1"));
    let mut msgs = Vec::new();
    loop {
        let m = next(&mut rx).await.expect("sink open");
        let lost = matches!(m, AudioMsg::DeviceLost { .. });
        msgs.push(m);
        if lost {
            break;
        }
    }
    assert!(!msgs
        .iter()
        .any(|m| matches!(m, AudioMsg::DeviceChanged { .. })));
    assert_eq!(
        samples_of(&msgs).len(),
        300,
        "captured audio flushed before the loss notice"
    );
    let report = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(report.frames_flushed, 0);
    assert!(collect(&mut rx).await.is_empty());
}

#[tokio::test]
async fn shutdown_joins_promptly_and_later_commands_fail_with_worker_gone() {
    let (src, fake) = source(48_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    let t = Instant::now();
    src.shutdown();
    assert!(t.elapsed() < callcore_audio::SHUTDOWN_JOIN_TIMEOUT);
    assert!(fake.is_dropped(0), "stream stopped on shutdown");
    assert!(collect(&mut rx).await.is_empty());
    let (tx, _rx) = mpsc::unbounded_channel();
    assert_eq!(src.start(tx).await, Err(AudioError::WorkerGone));
    assert_eq!(
        src.stop_and_drain(Duration::from_secs(2)).await,
        Err(AudioError::WorkerGone)
    );
    src.stop_discard().await; // never errors
    src.shutdown(); // idempotent
}

#[tokio::test]
async fn stops_are_idempotent_noops_when_idle() {
    let (src, _fake) = source(48_000, NO_POLL);
    src.stop_discard().await;
    src.stop_discard().await;
    let r = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(r, Default::default());
    let r = src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(r, Default::default());
}

#[tokio::test]
async fn start_while_capturing_replaces_the_old_capture() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    src.start(tx1).await.unwrap();
    let (tx2, _rx2) = mpsc::unbounded_channel();
    src.start(tx2).await.unwrap();
    assert!(collect(&mut rx1).await.is_empty());
    assert_eq!(fake.open_count(), 2);
    assert!(fake.is_dropped(0));
    assert!(fake.is_capturing());
}

#[tokio::test]
async fn multichannel_integer_input_is_downmixed() {
    let (src, fake) = source(16_000, NO_POLL);
    let (tx, mut rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    let stereo: Vec<i16> = (0..200).flat_map(|_| [16_384i16, 0]).collect();
    fake.feed_raw(2, Samples::I16(&stereo));
    src.stop_and_drain(Duration::from_secs(2)).await.unwrap();
    assert_eq!(
        samples_of(&collect(&mut rx).await),
        vec![f32_to_i16(0.25); 200]
    );
}

#[tokio::test]
async fn dropping_the_source_stops_capture() {
    let (src, fake) = source(48_000, NO_POLL);
    let (tx, _rx) = mpsc::unbounded_channel();
    src.start(tx).await.unwrap();
    drop(src);
    wait_until(|| fake.is_dropped(0)).await;
}

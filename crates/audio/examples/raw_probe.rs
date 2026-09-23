//! Manual QA only (never run by tests): drive the WASAPI backend directly,
//! bypassing the worker and DSP, for 3 s and count what the capture thread
//! delivers at the device rate. Device packets arrive with the mix-format
//! channel count; synthesized silence (device-flagged silent packets and
//! wall-clock gap filling) arrives as mono chunks, so the two are counted
//! separately.
//!
//!     cargo run -p callcore-audio --example raw_probe

#[cfg(windows)]
fn main() {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use callcore_audio::backend::CaptureBackend;
    use callcore_audio::dsp::Samples;
    use callcore_audio::wasapi::WasapiBackend;

    #[derive(Default)]
    struct Stats {
        device_chunks: AtomicUsize,
        device_frames: AtomicUsize,
        silence_chunks: AtomicUsize,
        silence_frames: AtomicUsize,
        peak_bits: AtomicU64,
    }

    let mut backend = WasapiBackend;
    println!("default device id: {:?}", backend.default_device_id());
    let stats = Arc::new(Stats::default());
    let st = stats.clone();
    let opened = backend
        .open(
            Box::new(move |chunk| {
                let Samples::F32(s) = chunk.samples else {
                    return;
                };
                let frames = s.len() / usize::from(chunk.channels.max(1));
                if chunk.channels == 1 {
                    st.silence_chunks.fetch_add(1, Ordering::Relaxed);
                    st.silence_frames.fetch_add(frames, Ordering::Relaxed);
                } else {
                    st.device_chunks.fetch_add(1, Ordering::Relaxed);
                    st.device_frames.fetch_add(frames, Ordering::Relaxed);
                }
                let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
                st.peak_bits
                    .fetch_max(f64::from(peak).to_bits(), Ordering::Relaxed);
            }),
            Box::new(|e| eprintln!("stream error: {e:?}")),
        )
        .expect("open");
    let rate = f64::from(opened.sample_rate);
    println!(
        "rate: {} Hz  device: {}",
        opened.sample_rate, opened.device_id
    );
    let mut stream = opened.stream;
    let t0 = Instant::now();
    stream.play().expect("play");
    std::thread::sleep(Duration::from_secs(3));
    drop(stream); // joins the capture thread; everything is delivered now
    let wall = t0.elapsed().as_secs_f64();
    let dev = stats.device_frames.load(Ordering::Relaxed);
    let sil = stats.silence_frames.load(Ordering::Relaxed);
    println!(
        "wall: {wall:.3} s  device packets: {} ({dev} frames, {:.3} s)  silence chunks: {} ({sil} frames, {:.3} s)",
        stats.device_chunks.load(Ordering::Relaxed),
        dev as f64 / rate,
        stats.silence_chunks.load(Ordering::Relaxed),
        sil as f64 / rate,
    );
    println!(
        "total: {:.3} s  peak |sample|: {:.4}",
        (dev + sil) as f64 / rate,
        f64::from_bits(stats.peak_bits.load(Ordering::Relaxed)),
    );
}

#[cfg(not(windows))]
fn main() {
    eprintln!("raw_probe needs Windows (WASAPI)");
}

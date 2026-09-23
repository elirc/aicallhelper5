//! Manual QA only (never run by tests): capture the real default render
//! device via WASAPI loopback for 3 s and print what arrived.
//!
//!     cargo run -p callcore-audio --example capture_probe
//!
//! Play some audio (a video, music) while it runs; peak RMS should be > 0.

use std::time::Duration;

use callcore_audio::LoopbackSource;
use callcore_contract::ports::{AudioMsg, AudioSource};

#[tokio::main]
async fn main() {
    let src = match LoopbackSource::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("worker failed: {e}");
            std::process::exit(1);
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    if let Err(e) = src.start(tx).await {
        eprintln!("start failed: {e}");
        src.shutdown();
        std::process::exit(1);
    }
    println!("capturing for 3 s ...");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let report = src
        .stop_and_drain(callcore_contract::config::AUDIO_DRAIN_TIMEOUT)
        .await;
    let (mut frames, mut samples, mut peak, mut notices) = (0usize, 0usize, 0f32, Vec::new());
    while let Some(msg) = rx.recv().await {
        match msg {
            AudioMsg::Frame(f) => {
                frames += 1;
                samples += f.samples.len();
                peak = peak.max(f.rms);
            }
            other => notices.push(other),
        }
    }
    println!("drain: {report:?}");
    println!(
        "frames: {frames}  samples: {samples} (~{:.2} s @16 kHz)  peak RMS: {peak:.4}",
        samples as f64 / 16_000.0
    );
    for n in notices {
        println!("notice: {n:?}");
    }
    src.shutdown();
}

//! Latency metrics (spec §9): exact under a paused clock.

mod support;

use std::time::Duration;

use callcore_contract::Metrics;
use support::*;

/// Record, wait `recording_for`, stop; drain takes `drain`, the STT flush
/// `flush`, the provider's first delta `first`.
async fn run(
    recording_for: Duration,
    drain: Duration,
    flush: Duration,
    first: Duration,
) -> Metrics {
    let h = Harness::new();
    h.audio.set_drain(drain, vec![frame(9)]);
    h.stt.set_flush(FlushMode::Auto {
        delay: flush,
        transcript: TRANSCRIPT.into(),
        late_failure: None,
    });
    h.provider
        .push_script(Script::ok_after(first, &["one ", "two"]));
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    tokio::time::sleep(recording_for).await;
    h.handle.stop_session(&id).await.unwrap();
    h.sink.wait_done(&id).await.metrics
}

#[tokio::test(start_paused = true)]
async fn metrics_are_exact_and_measured_from_stop_acceptance() {
    let m = run(secs(30), ms(250), ms(150), ms(400)).await;
    assert_eq!(
        m,
        Metrics {
            audio_drain_ms: 250,
            stt_finalize_ms: 150,
            first_token_ms: 800,
            total_ms: 800
        }
    );
}

#[tokio::test(start_paused = true)]
async fn injected_drain_delay_increases_first_token_ms_by_exactly_that_delay() {
    let base = run(secs(1), ms(0), ms(100), ms(300)).await;
    for d in [1u64, 250, 1234, 1999] {
        let m = run(secs(1), ms(d), ms(100), ms(300)).await;
        assert_eq!(
            m.first_token_ms,
            base.first_token_ms + d as u32,
            "drain {d} ms"
        );
        assert_eq!(m.total_ms, base.total_ms + d as u32);
        assert_eq!(m.audio_drain_ms, d as u32);
        assert_eq!(m.stt_finalize_ms, base.stt_finalize_ms);
    }
}

#[tokio::test(start_paused = true)]
async fn recording_length_does_not_count_toward_latency() {
    let short = run(ms(10), ms(100), ms(100), ms(100)).await;
    let long = run(secs(100), ms(100), ms(100), ms(100)).await;
    assert_eq!(short, long);
}

#[tokio::test(start_paused = true)]
async fn total_includes_streaming_after_the_first_token() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![
        Step::Sleep(ms(200)),
        Step::Delta("a".into()),
        Step::Sleep(ms(700)),
        Step::Delta("b".into()),
    ]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let m = h.sink.wait_done(&id).await.metrics;
    assert_eq!(
        m,
        Metrics {
            audio_drain_ms: 0,
            stt_finalize_ms: 0,
            first_token_ms: 200,
            total_ms: 900
        }
    );
}

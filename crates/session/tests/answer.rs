//! Answer streaming: retry policy, watchdogs, provider/STT failure mapping.

mod support;

use callcore_contract::ports::{ProviderFailure, ProviderFailureKind, SttEvent, SttFailureKind};
use callcore_contract::{copy, ErrorCode, Finish};
use support::*;

fn connect_failure() -> Step {
    Step::Return(Err(ProviderFailure {
        kind: ProviderFailureKind::Connect,
        message: "Could not reach Anthropic (connection refused).".into(),
    }))
}

#[tokio::test(start_paused = true)]
async fn retries_once_on_connect_before_any_delta_with_identical_bytes() {
    let h = Harness::new();
    h.provider
        .push_script(Script(vec![Step::Sleep(ms(100)), connect_failure()]));
    h.provider
        .push_script(Script::ok_after(ms(200), &["Hello ", "there."]));
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.answer, "Hello there.");
    assert_eq!(h.provider.stream_calls(), 2);
    assert_eq!(h.provider.builds(), 1, "the request is built ONCE");
    let reqs = h.provider.requests();
    assert_eq!(
        reqs[0], reqs[1],
        "the retry resends the byte-identical request"
    );
    // The retry stays inside the same first-token budget and clock.
    assert_eq!(done.metrics.first_token_ms, 300);
    assert_eq!(h.sink.kinds_dedup(&id).last(), Some(&"llm:done"));
}

#[tokio::test(start_paused = true)]
async fn second_connect_failure_is_not_retried_again() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![connect_failure()]));
    h.provider.push_script(Script(vec![connect_failure()]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::LlmHttp);
    assert_eq!(msg, "Could not reach Anthropic (connection refused).");
    assert_eq!(h.provider.stream_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn empty_answer_is_never_retried() {
    let h = Harness::new();
    h.provider.push_script(Script::fail(
        ProviderFailureKind::EmptyAnswer,
        "The answer provider returned an empty answer.",
    ));
    let id = h.handle.ask("q".into()).await.unwrap();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::LlmHttp);
    assert_eq!(h.provider.stream_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn whitespace_ok_outcome_is_an_error_not_a_blank_done() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![
        Step::Delta("  ".into()),
        Step::Return(Ok(callcore_contract::ports::StreamOutcome {
            finish: Finish::Complete,
            answer: "  ".into(),
        })),
    ]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::LlmHttp);
    assert!(!h.sink.kinds(&id).contains(&"llm:done"));
    assert_eq!(h.provider.stream_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn non_connect_failures_are_not_retried_and_map_to_the_closed_set() {
    let cases = [
        (
            ProviderFailureKind::Auth { status: 401 },
            ErrorCode::LlmAuth,
        ),
        (
            ProviderFailureKind::RateLimit { status: 429 },
            ErrorCode::LlmRateLimit,
        ),
        (
            ProviderFailureKind::Http { status: 500 },
            ErrorCode::LlmHttp,
        ),
        (ProviderFailureKind::StreamDrop, ErrorCode::LlmHttp),
        (ProviderFailureKind::ProviderError, ErrorCode::LlmHttp),
        (ProviderFailureKind::Incomplete, ErrorCode::LlmHttp),
        (ProviderFailureKind::ModelUnavailable, ErrorCode::LlmHttp),
        (ProviderFailureKind::Aborted, ErrorCode::Aborted),
    ];
    for (kind, code) in cases {
        let h = Harness::new();
        h.provider
            .push_script(Script::fail(kind, "specific copy (status quoted)"));
        let id = h.handle.ask("q".into()).await.unwrap();
        let (got, msg) = h.sink.wait_error(&id).await;
        assert_eq!(got, code, "{kind:?}");
        assert_eq!(msg, "specific copy (status quoted)");
        assert_eq!(h.provider.stream_calls(), 1, "{kind:?} must not be retried");
    }
}

#[tokio::test(start_paused = true)]
async fn finish_reason_is_passed_through() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![
        Step::Delta("cut".into()),
        Step::Return(Ok(callcore_contract::ports::StreamOutcome {
            finish: Finish::Truncated,
            answer: "cut".into(),
        })),
    ]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.finish, Finish::Truncated);
}

#[tokio::test(start_paused = true)]
async fn first_token_timeout_fires_at_ten_seconds_and_drops_the_stream() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![Step::Hang]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let t0 = tokio::time::Instant::now();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(t0.elapsed(), h.timeouts.llm_first_token);
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::LlmFirstTokenTimeout, copy::FIRST_TOKEN_TIMEOUT)
    );
    h.wait_until("stream dropped", || h.provider.live_streams() == 0)
        .await;
}

#[tokio::test(start_paused = true)]
async fn total_timeout_fires_at_sixty_seconds_even_while_streaming() {
    let h = Harness::new();
    let mut steps = Vec::new();
    for _ in 0..100 {
        steps.push(Step::Delta("word ".into()));
        steps.push(Step::Sleep(secs(1)));
    }
    h.provider.push_script(Script(steps));
    let id = h.handle.ask("q".into()).await.unwrap();
    let t0 = tokio::time::Instant::now();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(t0.elapsed(), h.timeouts.llm_total);
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::LlmTimeout, copy::TOTAL_TIMEOUT)
    );
    assert!(h.sink.kinds(&id).contains(&"llm:delta"));
    h.wait_until("stream dropped", || h.provider.live_streams() == 0)
        .await;
    assert_eq!(h.provider.stream_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_delta_disarms_the_first_token_watchdog() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![
        Step::Sleep(secs(9)),
        Step::Delta("a".into()),
        Step::Sleep(secs(20)),
        Step::Delta("b".into()),
        Step::Return(Ok(callcore_contract::ports::StreamOutcome {
            finish: Finish::Complete,
            answer: "ab".into(),
        })),
    ]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.answer, "ab");
    assert_eq!(done.metrics.first_token_ms, 9_000);
    assert_eq!(done.metrics.total_ms, 29_000);
}

// ── STT failures ──

#[tokio::test(start_paused = true)]
async fn stt_failure_before_flushed_is_stt_error() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    assert!(h.stt.emit(SttEvent::Failed(Harness::stt_failure(
        SttFailureKind::Server,
        "Deepgram closed the connection unexpectedly (code 1011)."
    ))));
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttError);
    assert!(msg.contains("1011"));
    assert_eq!(h.provider.stream_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn stt_bad_key_mid_stream_keeps_the_1008_message() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.stt.emit(SttEvent::Failed(Harness::stt_failure(
        SttFailureKind::BadKey,
        "Deepgram closed the connection (code 1008: invalid credentials)."
    ))));
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttConnect);
    assert!(msg.contains("1008"));
}

#[tokio::test(start_paused = true)]
async fn stt_event_stream_ending_before_flushed_is_stt_error() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    let id = h.start_recording().await;
    // The connector drops its event sender without a Flushed/Failed.
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    h.stt.end_events();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttError);
    assert_eq!(h.provider.stream_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn stt_send_failure_is_stt_error() {
    let h = Harness::new();
    let id = h.start_recording().await;
    h.stt.fail_next_send(Harness::stt_failure(
        SttFailureKind::Closed,
        "socket is gone",
    ));
    assert!(h.audio.push_frame(1));
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::SttError, "socket is gone")
    );
    h.wait_until("discard", || h.audio.calls().contains(&AudioCall::Discard))
        .await;
}

#[tokio::test(start_paused = true)]
async fn stt_connect_failure_is_stt_connect() {
    let h = Harness::new();
    h.stt
        .fail_next_connect(Harness::stt_failure(SttFailureKind::Connect, "dns failure"));
    let id = h.handle.start_session().await.unwrap();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::SttConnect, copy::STT_CONNECT)
    );
    h.wait_until("capture discarded", || !h.audio.capturing())
        .await;
}

#[tokio::test(start_paused = true)]
async fn stt_connect_timeout_is_stt_connect_after_five_seconds() {
    let h = Harness::new();
    h.stt.set_connect_delay(secs(3600));
    let id = h.handle.start_session().await.unwrap();
    let t0 = tokio::time::Instant::now();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttConnect);
    assert_eq!(t0.elapsed(), h.timeouts.stt_connect);
}

#[tokio::test(start_paused = true)]
async fn stop_during_connect_then_connect_timeout_is_stt_connect() {
    let h = Harness::new();
    h.stt.set_connect_delay(secs(3600));
    let id = h.handle.start_session().await.unwrap();
    h.sink.wait_kind(&id, "session:recording").await;
    h.handle.stop_session(&id).await.unwrap();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttConnect);
    assert_eq!(h.provider.stream_calls(), 0);
    assert!(!h.audio.calls().contains(&AudioCall::Drain));
}

#[tokio::test(start_paused = true)]
async fn finalize_timeout_is_stt_timeout_after_five_seconds() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    let t0 = tokio::time::Instant::now();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::SttTimeout, copy::STT_TIMEOUT)
    );
    assert!(t0.elapsed() <= h.timeouts.stt_finalize);
    assert_eq!(h.provider.stream_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn prompt_uses_the_flushed_transcript_not_the_last_partial() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.stt.transcript("Tell me", false));
    h.sink.wait_kind(&id, "stt:partial").await;
    h.handle.stop_session(&id).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.transcript, TRANSCRIPT);
    assert!(h.provider.prompts()[0].user_message.contains(TRANSCRIPT));
}

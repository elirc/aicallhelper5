//! Session machine invariants (spec §5 1–11) and core-side lessons (§14).

mod support;

use callcore_contract::ports::{AudioMsg, ProviderFailureKind, SttEvent, SttFailureKind};
use callcore_contract::{copy, ErrorCode, Phase, SessionId};
use support::*;

/// Assert that no session in `ids` ever got more than one terminal event.
fn assert_at_most_one_terminal(h: &Harness, ids: &[&SessionId]) {
    for id in ids {
        assert!(
            h.sink.terminals(id).len() <= 1,
            "{id}: {:#?}",
            h.sink.for_session(id)
        );
    }
}

// ── 1. one live session; a new start/ask supersedes ──

#[tokio::test(start_paused = true)]
async fn inv1_new_start_supersedes_the_live_session() {
    let h = Harness::new();
    let s1 = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    let s2 = h.start_recording().await;
    assert_eq!(h.handle.status().id, Some(s2.clone()));
    // Old capture discarded before the new one started (lane ordering).
    assert_eq!(
        h.audio.calls(),
        vec![AudioCall::Start, AudioCall::Discard, AudioCall::Start]
    );
    assert!(h.stt.log(0).contains(&SttLog::Abort));
    assert!(!h.stt.log(0).contains(&SttLog::Close));
    // Stopping the superseded session is refused; the new one still works.
    let err = h.handle.stop_session(&s1).await.unwrap_err();
    assert_eq!(err.message, copy::STOP_NOT_TAKEN);
    assert!(h.audio.push_frame(2));
    h.handle.stop_session(&s2).await.unwrap();
    h.sink.wait_done(&s2).await;
    assert_eq!(h.stt.frames_sent(1), vec![2]);
    assert_at_most_one_terminal(&h, &[&s1, &s2]);
}

#[tokio::test(start_paused = true)]
async fn inv1_superseded_session_emits_nothing_further() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![Step::Hang]));
    let s1 = h.handle.ask("first".into()).await.unwrap();
    h.settle().await;
    let before = h.sink.for_session(&s1).len();
    let s2 = h.handle.ask("second".into()).await.unwrap();
    h.sink.wait_done(&s2).await;
    // Let every s1 watchdog deadline pass: they were dropped with s1.
    tokio::time::sleep(secs(300)).await;
    assert_eq!(h.sink.for_session(&s1).len(), before);
    assert!(!h.sink.has_terminal(&s1));
    h.wait_until("s1 stream dropped", || h.provider.live_streams() == 0)
        .await;
}

#[tokio::test(start_paused = true)]
async fn inv1_supersede_during_audio_start_discards_after_start() {
    let h = Harness::new();
    h.audio.set_start_delay(ms(500));
    let s1 = h.handle.start_session().await.unwrap();
    h.settle().await;
    let s2 = h.handle.start_session().await.unwrap();
    h.sink.wait_kind(&s2, "session:recording").await;
    assert!(
        h.sink.for_session(&s1).is_empty(),
        "s1's late start result is stale"
    );
    assert_eq!(
        h.audio.calls(),
        vec![AudioCall::Start, AudioCall::Discard, AudioCall::Start]
    );
    assert!(h.audio.push_frame(7));
    h.handle.stop_session(&s2).await.unwrap();
    h.sink.wait_done(&s2).await;
}

// ── 2. a start that lost the race never installs ──

#[tokio::test(start_paused = true)]
async fn inv2_stalled_key_read_loses_to_newer_start() {
    let h = Harness::new();
    h.settings.arm_gate();
    let handle = h.handle.clone();
    let stale = tokio::spawn(async move { handle.start_session().await });
    // Wait (real time: the key read runs on a blocking thread) until it stalls.
    let t0 = std::time::Instant::now();
    while !h.settings.gate_blocked() {
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(10),
            "gate never blocked"
        );
        tokio::task::yield_now().await;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let winner = h.start_recording().await;
    assert_eq!(winner, SessionId("s1".into()));
    h.settings.release_gate();
    let err = stale.await.unwrap().unwrap_err();
    assert_eq!(err.code, ErrorCode::Aborted);
    // Never installed, never superseded the winner.
    assert_eq!(h.handle.status().id, Some(winner.clone()));
    assert_eq!(h.handle.status().phase, Phase::Recording);
    assert_eq!(h.audio.calls(), vec![AudioCall::Start]);
    assert_eq!(h.stt.connects(), 1);
    assert!(h.audio.push_frame(1));
    h.handle.stop_session(&winner).await.unwrap();
    h.sink.wait_done(&winner).await;
    // The next session number continues from the winner.
    let next = h.handle.ask("q".into()).await.unwrap();
    assert_eq!(next, SessionId("s2".into()));
}

#[tokio::test(start_paused = true)]
async fn inv2_late_cancel_of_an_old_command_only_hits_its_own_session() {
    let h = Harness::new();
    let s1 = h.start_recording().await;
    let s2 = h.start_recording().await;
    // e.g. the page's timed-out start for s1 cleans up late.
    h.handle.cancel_session(&s1);
    h.settle().await;
    assert_eq!(h.handle.status().id, Some(s2.clone()));
    assert!(h.audio.push_frame(5));
    h.handle.stop_session(&s2).await.unwrap();
    h.sink.wait_done(&s2).await;
}

// ── 3. events for a stale session are dropped ──

#[tokio::test(start_paused = true)]
async fn inv3_late_results_of_a_superseded_session_are_dropped() {
    let h = Harness::new();
    let s1 = h.start_recording().await;
    let s1_events = h.sink.for_session(&s1).len();
    let s2 = h.start_recording().await;
    // Late STT results / late audio callback for s1.
    let _ = h.stt.emit_on(
        0,
        SttEvent::Transcript {
            text: "late".into(),
            is_final: true,
        },
    );
    let _ = h.stt.emit_on(
        0,
        SttEvent::Flushed {
            transcript: "late".into(),
        },
    );
    assert!(
        !h.audio.push_to_old_sink(0, AudioMsg::Frame(frame(66))),
        "old sink is closed"
    );
    h.settle().await;
    assert_eq!(h.sink.for_session(&s1).len(), s1_events);
    assert!(h.stt.frames_sent(1).is_empty());
    assert!(h.stt.frames_sent(0).is_empty());
    h.handle.cancel_session(&s2);
}

#[tokio::test(start_paused = true)]
async fn inv3_late_provider_result_after_cancel_is_ignored() {
    let h = Harness::new();
    h.provider
        .push_script(Script::ok_after(secs(2), &["late answer"]));
    let s1 = h.handle.ask("q".into()).await.unwrap();
    h.settle().await;
    h.handle.cancel_session(&s1);
    h.settle().await;
    assert_eq!(h.handle.status(), callcore_contract::SessionStatus::idle());
    tokio::time::sleep(secs(120)).await;
    assert_eq!(h.sink.kinds(&s1), vec!["stt:partial"]);
    assert_eq!(h.provider.live_streams(), 0);
}

// ── 4. capture cutoff; CloseStream last; cancel = discard ──

#[tokio::test(start_paused = true)]
async fn inv4_capture_cutoff_and_close_stream_is_last() {
    let h = Harness::new();
    // The drain delivers the final partial frame(s) just before completing.
    h.audio.set_drain(ms(300), vec![frame(8), frame(9)]);
    let id = h.start_recording().await;
    for t in 1..=3 {
        assert!(h.audio.push_frame(t));
    }
    h.handle.stop_session(&id).await.unwrap();
    // Frames that arrive during the drain are pre-stop audio: accepted.
    assert!(h.audio.push_frame(4));
    h.sink.wait_done(&id).await;
    // After the cutoff the sink is gone: a late callback cannot post.
    assert!(!h.audio.push_to_old_sink(0, AudioMsg::Frame(frame(99))));
    let log = h.stt.log(0);
    let close_at = log
        .iter()
        .position(|l| *l == SttLog::Close)
        .expect("CloseStream sent");
    assert!(
        log[close_at + 1..]
            .iter()
            .all(|l| !matches!(l, SttLog::Frame(_))),
        "{log:?}"
    );
    assert_eq!(h.stt.frames_sent(0), vec![1, 2, 3, 4, 8, 9]);
    assert_eq!(log.iter().filter(|l| **l == SttLog::Close).count(), 1);
    // No level meter after Stop.
    let levels = h
        .sink
        .kinds(&id)
        .iter()
        .filter(|k| **k == "audio:level")
        .count();
    assert_eq!(levels, 3);
}

#[tokio::test(start_paused = true)]
async fn inv4_cancel_uses_discard_never_drain_or_close() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    h.handle.cancel_session(&id);
    h.wait_until("discarded", || {
        h.audio.calls().contains(&AudioCall::Discard)
    })
    .await;
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
    h.wait_until("socket aborted", || h.stt.log(0).contains(&SttLog::Abort))
        .await;
    assert!(!h.stt.log(0).contains(&SttLog::Close));
    assert!(!h.audio.capturing());
    tokio::time::sleep(secs(300)).await;
    assert!(!h.sink.has_terminal(&id), "cancel emits nothing");
}

#[tokio::test(start_paused = true)]
async fn inv4_cancel_during_finalize_discards_and_emits_nothing() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    h.handle.cancel_session(&id);
    h.settle().await;
    let _ = h.stt.emit_on(
        0,
        SttEvent::Flushed {
            transcript: "x".into(),
        },
    );
    tokio::time::sleep(secs(60)).await;
    assert!(!h.sink.has_terminal(&id));
    assert_eq!(h.provider.stream_calls(), 0);
}

// ── 5. a late STT close after the final transcript never kills the answer ──

#[tokio::test(start_paused = true)]
async fn inv5_late_stt_failure_after_flushed_is_ignored() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Auto {
        delay: ms(50),
        transcript: TRANSCRIPT.into(),
        late_failure: Some(Harness::stt_failure(
            SttFailureKind::Server,
            "socket closed 1006",
        )),
    });
    h.provider
        .push_script(Script::ok_after(secs(2), &["streaming ", "answer"]));
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.answer, "streaming answer");
    assert_eq!(h.sink.terminals(&id).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn inv5_stt_stream_end_after_flushed_is_ignored() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    h.provider.push_script(Script::ok_after(secs(1), &["ok"]));
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    assert!(h.stt.emit(SttEvent::Flushed {
        transcript: TRANSCRIPT.into()
    }));
    // The server's close / a transport error lands while the answer streams.
    let _ = h.stt.emit(SttEvent::Failed(Harness::stt_failure(
        SttFailureKind::Closed,
        "gone",
    )));
    h.sink.wait_done(&id).await;
    assert_eq!(h.sink.terminals(&id).len(), 1);
}

// ── 6. never retry after a delta ──

#[tokio::test(start_paused = true)]
async fn inv6_no_retry_after_a_delta() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![
        Step::Delta("partial ".into()),
        Step::Return(Err(callcore_contract::ports::ProviderFailure {
            kind: ProviderFailureKind::Connect,
            message: "connection reset".into(),
        })),
    ]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::LlmHttp, "connection reset")
    );
    assert_eq!(h.provider.stream_calls(), 1);
    assert_eq!(
        h.sink.kinds(&id),
        vec!["stt:partial", "llm:delta", "session:error"]
    );
}

// ── 7. never call the LLM on an empty prompt ──

#[tokio::test(start_paused = true)]
async fn inv7_whitespace_transcript_is_no_speech_and_provider_never_called() {
    for transcript in ["", "   ", "\n\t"] {
        let h = Harness::new();
        h.stt.set_flush_transcript(transcript);
        let id = h.start_recording().await;
        h.handle.stop_session(&id).await.unwrap();
        let (code, msg) = h.sink.wait_error(&id).await;
        assert_eq!((code, msg.as_str()), (ErrorCode::NoSpeech, copy::NO_SPEECH));
        assert_eq!(h.provider.builds(), 0);
        assert_eq!(h.provider.stream_calls(), 0);
    }
}

// ── 8. at most one terminal event per session ──

#[tokio::test(start_paused = true)]
async fn inv8_watchdog_then_late_provider_result_is_one_terminal() {
    let h = Harness::new();
    h.provider
        .push_script(Script::ok_after(secs(11), &["too late"]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::LlmFirstTokenTimeout);
    tokio::time::sleep(secs(120)).await;
    assert_eq!(h.sink.terminals(&id).len(), 1);
    assert!(!h.sink.kinds(&id).contains(&"llm:delta"));
}

#[tokio::test(start_paused = true)]
async fn inv8_stt_failure_racing_finalize_is_one_terminal() {
    let h = Harness::new();
    h.stt.set_flush(FlushMode::Manual);
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.wait_until("close sent", || h.stt.log(0).contains(&SttLog::Close))
        .await;
    let _ = h.stt.emit(SttEvent::Failed(Harness::stt_failure(
        SttFailureKind::Server,
        "500",
    )));
    let _ = h.stt.emit(SttEvent::Flushed {
        transcript: TRANSCRIPT.into(),
    });
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttError);
    tokio::time::sleep(secs(60)).await;
    assert_eq!(h.sink.terminals(&id).len(), 1);
    assert_eq!(h.provider.stream_calls(), 0);
}

// ── 9. timers owned by the session, cleared on every exit; no leaked tasks ──

#[tokio::test(start_paused = true)]
async fn inv9_no_timer_fires_and_no_task_survives_after_done() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    h.handle.stop_session(&id).await.unwrap();
    h.sink.wait_done(&id).await;
    let n = h.sink.len();
    // Past the cap, finalize, first-token and total deadlines.
    tokio::time::sleep(secs(600)).await;
    assert_eq!(h.sink.len(), n);
    h.wait_until("tasks gone", || {
        h.audio.all_receivers_dropped()
            && h.stt.all_readers_dropped()
            && h.provider.all_delta_receivers_dropped()
            && h.provider.live_streams() == 0
    })
    .await;
    assert_eq!(h.stt.log(0).last(), Some(&SttLog::Dropped));
}

#[tokio::test(start_paused = true)]
async fn inv9_no_task_survives_cancel_in_any_phase() {
    // Recording.
    let h = Harness::new();
    let id = h.start_recording().await;
    h.handle.cancel_session(&id);
    h.wait_until("recording tasks gone", || {
        h.audio.all_receivers_dropped() && h.stt.all_readers_dropped()
    })
    .await;
    assert_eq!(h.stt.log(0).last(), Some(&SttLog::Dropped));

    // Answering with a hung provider.
    h.provider.push_script(Script(vec![Step::Hang]));
    let id = h.handle.ask("q".into()).await.unwrap();
    h.wait_until("streaming", || h.provider.live_streams() == 1)
        .await;
    h.handle.cancel_session(&id);
    h.wait_until("stream dropped", || {
        h.provider.live_streams() == 0 && h.provider.all_delta_receivers_dropped()
    })
    .await;
    tokio::time::sleep(secs(600)).await;
    assert!(!h.sink.has_terminal(&id), "no watchdog fired after cancel");
}

#[tokio::test(start_paused = true)]
async fn inv9_no_task_survives_an_error_exit() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.stt.emit(SttEvent::Failed(Harness::stt_failure(
        SttFailureKind::Server,
        "boom"
    ))));
    let (code, _) = h.sink.wait_error(&id).await;
    assert_eq!(code, ErrorCode::SttError);
    // Capture was running: discard-stop, never drain.
    h.wait_until("discard", || h.audio.calls().contains(&AudioCall::Discard))
        .await;
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
    h.wait_until("tasks gone", || {
        h.audio.all_receivers_dropped() && h.stt.all_readers_dropped()
    })
    .await;
    tokio::time::sleep(secs(600)).await;
    assert_eq!(h.sink.terminals(&id).len(), 1);
}

// ── 10. cancel is idempotent, never errors, affects only its id ──

#[tokio::test(start_paused = true)]
async fn inv10_cancel_is_idempotent_and_scoped() {
    let h = Harness::new();
    // Unknown id / nothing live: a no-op.
    h.handle.cancel_session(&SessionId("s99".into()));
    let s1 = h.start_recording().await;
    h.handle.cancel_session(&SessionId("s99".into()));
    h.settle().await;
    assert_eq!(h.handle.status().id, Some(s1.clone()));
    h.handle.cancel_session(&s1);
    h.handle.cancel_session(&s1);
    h.handle.cancel_session(&s1);
    h.settle().await;
    assert_eq!(h.handle.status(), callcore_contract::SessionStatus::idle());
    h.wait_until("one discard", || {
        h.audio.calls().contains(&AudioCall::Discard)
    })
    .await;
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
    // Cancel after a finished session is harmless too.
    let s2 = h.handle.ask("q".into()).await.unwrap();
    h.sink.wait_done(&s2).await;
    h.handle.cancel_session(&s2);
    h.handle.cancel_session(&s1);
    h.settle().await;
    assert_eq!(h.sink.terminals(&s2).len(), 1);
    assert!(!h.sink.has_terminal(&s1));
}

// ── 11. one deadline for the UI countdown and the core cap ──

#[tokio::test(start_paused = true)]
async fn inv11_status_deadline_equals_event_deadline_and_cap_fires_there() {
    let h = Harness::new();
    tokio::time::sleep(ms(1234)).await;
    let id = h.start_recording().await;
    let (deadline_ms, cap_ms) = match &h.sink.for_session(&id)[0] {
        callcore_contract::CoreEvent::SessionRecording {
            deadline_ms,
            cap_ms,
            ..
        } => (*deadline_ms, *cap_ms),
        e => panic!("{e:?}"),
    };
    let info = h.handle.status().recording.unwrap();
    assert_eq!((info.deadline_ms, info.cap_ms), (deadline_ms, cap_ms));
    h.sink.wait_kind(&id, "session:autostopped").await;
    assert_eq!(h.clock.epoch_ms(), deadline_ms);
}

// ── §14 lesson 11: device-open failure beats no_speech ──

#[tokio::test(start_paused = true)]
async fn lesson11_device_open_failure_with_early_stop_reports_device_error() {
    let h = Harness::new();
    h.audio.set_start_delay(ms(400));
    h.audio
        .fail_next_start(callcore_contract::ports::AudioError::DeviceOpen(
            copy::DEVICE_OPEN.into(),
        ));
    h.stt.set_flush_transcript("");
    let id = h.handle.start_session().await.unwrap();
    h.wait_until("connected", || h.stt.connected()).await;
    // Early Stop, before the device open has failed.
    h.handle.stop_session(&id).await.unwrap();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::Internal, copy::DEVICE_OPEN)
    );
    tokio::time::sleep(secs(60)).await;
    assert_eq!(h.sink.terminals(&id).len(), 1);
    assert_eq!(h.provider.stream_calls(), 0);
    assert!(!h.sink.kinds(&id).contains(&"session:recording"));
}

#[tokio::test(start_paused = true)]
async fn device_open_failure_while_recording_errors_immediately() {
    let h = Harness::new();
    h.audio
        .fail_next_start(callcore_contract::ports::AudioError::DeviceOpen(
            copy::DEVICE_OPEN.into(),
        ));
    let id = h.handle.start_session().await.unwrap();
    let (code, msg) = h.sink.wait_error(&id).await;
    assert_eq!(
        (code, msg.as_str()),
        (ErrorCode::Internal, copy::DEVICE_OPEN)
    );
    assert_eq!(h.handle.status(), callcore_contract::SessionStatus::idle());
    // No drain or discard for a device that never opened.
    assert_eq!(h.audio.calls(), vec![AudioCall::Start]);
}

// ── lesson 12: keys and profile text never reach events ──

#[tokio::test(start_paused = true)]
async fn secrets_and_profile_text_never_appear_in_events() {
    let h = Harness::new();
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    h.sink.wait_done(&id).await;
    h.provider.push_script(Script::fail(
        ProviderFailureKind::Auth { status: 401 },
        "Anthropic rejected the API key (401)",
    ));
    let id = h.handle.ask("q".into()).await.unwrap();
    h.sink.wait_error(&id).await;
    let all = serialized(&h.sink.events());
    for needle in [DG_KEY, LLM_KEY, RESUME] {
        assert!(!all.contains(needle), "{needle} leaked");
    }
}

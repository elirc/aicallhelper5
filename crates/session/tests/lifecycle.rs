//! Actor lifecycle: shutdown, handle drop, and a real-time multi-thread smoke.

mod support;

use std::time::Duration;

use callcore_contract::{ErrorCode, SessionId, SessionStatus};
use callcore_session::SHUTTING_DOWN;
use support::*;

#[tokio::test(start_paused = true)]
async fn shutdown_discards_the_live_session_and_later_calls_fail() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    h.handle.shutdown().await;
    // The discard finished before shutdown returned.
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
    assert!(!h.audio.capturing());
    h.wait_until("socket aborted", || h.stt.log(0).contains(&SttLog::Abort))
        .await;
    assert!(!h.stt.log(0).contains(&SttLog::Close));
    assert_eq!(h.handle.status(), SessionStatus::idle());

    for err in [
        h.handle.start_session().await.unwrap_err(),
        h.handle.ask("q".into()).await.unwrap_err(),
        h.handle.stop_session(&id).await.unwrap_err(),
    ] {
        assert_eq!(
            (err.code, err.message.as_str()),
            (ErrorCode::Internal, SHUTTING_DOWN)
        );
    }
    h.handle.cancel_session(&id);
    h.handle.shutdown().await;
    tokio::time::sleep(secs(300)).await;
    assert!(
        !h.sink.has_terminal(&id),
        "shutdown emits nothing for the session"
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_answering_drops_the_provider_stream() {
    let h = Harness::new();
    h.provider.push_script(Script(vec![Step::Hang]));
    let id = h.handle.ask("q".into()).await.unwrap();
    h.wait_until("streaming", || h.provider.live_streams() == 1)
        .await;
    h.handle.shutdown().await;
    h.wait_until("stream dropped", || h.provider.live_streams() == 0)
        .await;
    tokio::time::sleep(secs(120)).await;
    assert!(!h.sink.has_terminal(&id));
}

#[tokio::test(start_paused = true)]
async fn shutdown_with_nothing_live_is_quick_and_idempotent() {
    let h = Harness::new();
    h.handle.shutdown().await;
    h.handle.shutdown().await;
    assert!(h.audio.calls().is_empty());
    assert_eq!(h.handle.status(), SessionStatus::idle());
}

#[tokio::test(start_paused = true)]
async fn dropping_every_handle_stops_the_actor_and_discards() {
    let h = Harness::new();
    let _id = h.start_recording().await;
    let Harness { handle, audio, .. } = h;
    let clone = handle.clone();
    drop(handle);
    tokio::task::yield_now().await;
    assert!(audio.capturing(), "one clone is still alive");
    drop(clone);
    for _ in 0..1000 {
        if audio.calls().contains(&AudioCall::Discard) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
}

#[tokio::test(start_paused = true)]
async fn stop_of_unknown_or_ask_session_is_not_taken() {
    let h = Harness::new();
    let err = h
        .handle
        .stop_session(&SessionId("s1".into()))
        .await
        .unwrap_err();
    assert_eq!(err.message, callcore_contract::copy::STOP_NOT_TAKEN);
    h.provider.push_script(Script(vec![Step::Hang]));
    let id = h.handle.ask("q".into()).await.unwrap();
    let err = h.handle.stop_session(&id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    // Double stop: the second is refused, the first proceeds.
    h.handle.cancel_session(&id);
    let id = h.start_recording().await;
    h.handle.stop_session(&id).await.unwrap();
    assert!(h.handle.stop_session(&id).await.is_err());
    h.sink.wait_done(&id).await;
    assert_eq!(
        h.audio
            .calls()
            .iter()
            .filter(|c| **c == AudioCall::Drain)
            .count(),
        1
    );
}

/// Real clock + multi-thread runtime: nothing depends on current-thread
/// scheduling. Bounded waits, no fixed sleeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_time_multi_thread_smoke() {
    let h = Harness::new();
    let bound = Duration::from_secs(20);
    for round in 0..5 {
        let id = h.handle.start_session().await.unwrap();
        let id2 = id.clone();
        h.sink
            .wait_for_within(bound, "recording", move |e| {
                e.iter()
                    .any(|e| e.session_id() == Some(&id2) && kind(e) == "session:recording")
            })
            .await;
        for t in 0..10 {
            assert!(h.audio.push_frame(t));
        }
        h.handle.stop_session(&id).await.unwrap();
        let id3 = id.clone();
        h.sink
            .wait_for_within(bound, "done", move |e| {
                e.iter()
                    .any(|e| e.session_id() == Some(&id3) && is_terminal(e))
            })
            .await;
        assert!(
            matches!(
                h.sink.terminals(&id)[0],
                callcore_contract::CoreEvent::LlmDone { .. }
            ),
            "round {round}: {:?}",
            h.sink.terminals(&id)
        );
        assert_eq!(h.stt.frames_sent(round).len(), 10);
    }
    // Rapid supersede storm ends with exactly one live session.
    let mut last = None;
    for _ in 0..20 {
        last = Some(h.handle.start_session().await.unwrap());
    }
    let last = last.unwrap();
    assert_eq!(h.handle.status().id, Some(last.clone()));
    h.handle.shutdown().await;
}

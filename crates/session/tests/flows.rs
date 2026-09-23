//! User flows (spec §4): record -> stop -> answer, stop during connect,
//! pre-connect buffering, record cap, device loss/change, ask, keys, status.

mod support;

use callcore_contract::ports::AudioMsg;
use callcore_contract::{
    copy, AnswerStyle, CallType, CoreEvent, DeviceNoticeKind, ErrorCode, Finish, Phase, SessionId,
};
use callcore_prompt::{build_prompt, PromptInput};
use support::*;

#[tokio::test(start_paused = true)]
async fn happy_path_record_stop_done_exact_sequence() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert_eq!(id, SessionId("s1".into()));
    for tag in 1..=3 {
        assert!(h.audio.push_frame(tag));
    }
    let id2 = id.clone();
    h.sink
        .wait_for("3 levels", move |e| {
            e.iter()
                .filter(|e| e.session_id() == Some(&id2) && kind(e) == "audio:level")
                .count()
                == 3
        })
        .await;
    assert!(h.stt.transcript("Tell me", false));
    h.sink.wait_kind(&id, "stt:partial").await;

    h.handle.stop_session(&id).await.expect("stop taken");
    let done = h.sink.wait_done(&id).await;

    assert_eq!(
        h.sink.kinds(&id),
        vec![
            "session:recording",
            "audio:level",
            "audio:level",
            "audio:level",
            "stt:partial",
            "stt:partial",
            "llm:delta",
            "llm:delta",
            "llm:delta",
            "llm:done",
        ]
    );
    // The flushed transcript is re-emitted once as a final partial.
    let partials: Vec<_> = h
        .sink
        .for_session(&id)
        .into_iter()
        .filter_map(|e| match e {
            CoreEvent::SttPartial { text, is_final, .. } => Some((text, is_final)),
            _ => None,
        })
        .collect();
    assert_eq!(
        partials,
        vec![("Tell me".into(), false), (TRANSCRIPT.into(), true)]
    );
    assert_eq!(done.transcript, TRANSCRIPT);
    assert_eq!(done.answer, DEFAULT_ANSWER.concat());
    assert_eq!(done.finish, Finish::Complete);
    assert_eq!(done.call_type, CallType::Behavioral);

    // Frames, then CloseStream, then the socket is released.
    assert_eq!(
        h.stt.log(0),
        vec![
            SttLog::Frame(1),
            SttLog::Frame(2),
            SttLog::Frame(3),
            SttLog::Close,
            SttLog::Abort,
            SttLog::Dropped
        ]
    );
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Drain]);
    assert_eq!(h.stt.keys(), vec![DG_KEY.to_string()]);
    assert_eq!(h.provider.keys(), vec![LLM_KEY.to_string()]);
    assert_eq!(h.provider.builds(), 1);
    assert_eq!(h.provider.stream_calls(), 1);
    // Pre-warmed on Record and again on Stop.
    assert_eq!(h.provider.prewarms(), 2);
    assert_eq!(h.handle.status(), callcore_contract::SessionStatus::idle());
}

#[tokio::test(start_paused = true)]
async fn prompt_is_built_from_profile_style_and_call_type_at_answer_time() {
    let h = Harness::new();
    let id = h.start_recording().await;
    // Style and call type flipped mid-recording apply to this answer.
    h.settings.update_config(|c| {
        c.style = AnswerStyle::Brief;
        c.call_type = CallType::SystemDesign;
    });
    h.handle.stop_session(&id).await.unwrap();
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.call_type, CallType::SystemDesign);

    let cfg = h.settings.config();
    let expected = build_prompt(
        &PromptInput {
            call_type: CallType::SystemDesign,
            style: AnswerStyle::Brief,
            resume: &cfg.profile.resume,
            job_description: &cfg.profile.job_description,
            focus: &cfg.profile.focus,
            notes: &cfg.profile.notes,
        },
        TRANSCRIPT,
    );
    assert_eq!(h.provider.prompts(), vec![expected]);
}

#[tokio::test(start_paused = true)]
async fn stop_during_connect_waits_for_socket_then_drains() {
    let h = Harness::new();
    h.stt.set_connect_delay(ms(1500));
    h.audio.set_drain(ms(200), vec![frame(9)]);
    let id = h.handle.start_session().await.unwrap();
    h.sink.wait_kind(&id, "session:recording").await;
    assert!(h.audio.push_frame(1));
    assert_eq!(h.handle.status().phase, Phase::Recording);

    // Stop while the socket is still connecting: accepted.
    h.handle
        .stop_session(&id)
        .await
        .expect("stop during connect is taken");
    assert_eq!(h.handle.status().phase, Phase::Finalizing);
    // Capture continues until the drain, which only starts after the socket.
    assert!(h.audio.push_frame(2));
    h.settle().await;
    assert!(
        !h.audio.calls().contains(&AudioCall::Drain),
        "drain must wait for the socket"
    );

    let done = h.sink.wait_done(&id).await;
    assert_eq!(h.stt.frames_sent(0), vec![1, 2, 9]);
    assert_eq!(h.stt.log(0)[3], SttLog::Close);
    // audioDrainMs is the drain only, never the ~1.5 s connect wait...
    assert_eq!(done.metrics.audio_drain_ms, 200);
    // ...but the user-facing latency includes both.
    assert!(
        done.metrics.first_token_ms >= 1500 + 200,
        "{:?}",
        done.metrics
    );
}

#[tokio::test(start_paused = true)]
async fn pre_connect_buffer_caps_at_120_frames_dropping_oldest() {
    let h = Harness::new();
    h.stt.set_connect_delay(secs(3));
    let id = h.handle.start_session().await.unwrap();
    h.sink.wait_kind(&id, "session:recording").await;
    for tag in 0..130 {
        assert!(h.audio.push_frame(tag));
    }
    let id2 = id.clone();
    h.sink
        .wait_for("130 levels", move |e| {
            e.iter()
                .filter(|e| e.session_id() == Some(&id2) && kind(e) == "audio:level")
                .count()
                == 130
        })
        .await;
    assert!(!h.stt.connected());
    h.wait_until("connected", || h.stt.connected()).await;
    h.wait_until("buffer flushed", || h.stt.frames_sent(0).len() == 120)
        .await;
    // Live frames after the flush follow the buffered ones.
    assert!(h.audio.push_frame(500));
    h.wait_until("live frame", || h.stt.frames_sent(0).len() == 121)
        .await;
    let sent = h.stt.frames_sent(0);
    let expected: Vec<i16> = (10..130).chain([500]).collect();
    assert_eq!(sent, expected);
}

#[tokio::test(start_paused = true)]
async fn record_cap_autostops_then_answers() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    let done = h.sink.wait_done(&id).await;
    let deadline_ms = match &h.sink.for_session(&id)[0] {
        CoreEvent::SessionRecording { deadline_ms, .. } => *deadline_ms,
        e => panic!("{e:?}"),
    };
    // Autostopped exactly at the advertised deadline (the answer is instant).
    assert_eq!(h.clock.epoch_ms(), deadline_ms);
    let kinds = h.sink.kinds_dedup(&id);
    assert_eq!(
        kinds,
        vec![
            "session:recording",
            "audio:level",
            "session:autostopped",
            "stt:partial",
            "llm:delta",
            "llm:done"
        ]
    );
    assert_eq!(done.transcript, TRANSCRIPT);
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Drain]);
    // Stop after the autostop is no longer taken.
    let err = h.handle.stop_session(&id).await.unwrap_err();
    assert_eq!(err.message, copy::STOP_NOT_TAKEN);
}

#[tokio::test(start_paused = true)]
async fn device_lost_autostops_and_answers_with_captured_audio() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    assert!(h.audio.push(AudioMsg::DeviceLost {
        message: "unplugged".into()
    }));
    let done = h.sink.wait_done(&id).await;
    assert_eq!(done.transcript, TRANSCRIPT);
    let device: Vec<_> = h
        .sink
        .for_session(&id)
        .into_iter()
        .filter_map(|e| match e {
            CoreEvent::AudioDevice { kind, message, .. } => Some((kind, message)),
            _ => None,
        })
        .collect();
    assert_eq!(
        device,
        vec![(DeviceNoticeKind::Lost, copy::DEVICE_LOST.to_string())]
    );
    assert!(!h.sink.kinds(&id).contains(&"session:autostopped"));
    assert_eq!(h.stt.frames_sent(0), vec![1]);
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Drain]);
}

#[tokio::test(start_paused = true)]
async fn device_changed_only_notifies() {
    let h = Harness::new();
    let id = h.start_recording().await;
    assert!(h.audio.push(AudioMsg::DeviceChanged {
        message: "new default".into()
    }));
    h.sink.wait_kind(&id, "audio:device").await;
    assert_eq!(h.handle.status().phase, Phase::Recording);
    assert!(h.audio.push_frame(4));
    h.handle.stop_session(&id).await.unwrap();
    h.sink.wait_done(&id).await;
    let notice = h
        .sink
        .for_session(&id)
        .into_iter()
        .find_map(|e| match e {
            CoreEvent::AudioDevice { kind, message, .. } => Some((kind, message)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        notice,
        (DeviceNoticeKind::Changed, copy::DEVICE_CHANGED.to_string())
    );
    assert_eq!(h.stt.frames_sent(0), vec![4]);
}

#[tokio::test(start_paused = true)]
async fn ask_answers_directly_with_zero_audio_metrics() {
    let h = Harness::new();
    h.provider
        .push_script(Script::ok_after(ms(300), &["Sure."]));
    let id = h.handle.ask("  What is Rust?  ".into()).await.unwrap();
    assert_eq!(h.handle.status().phase, Phase::Answering);
    let done = h.sink.wait_done(&id).await;
    assert_eq!(
        h.sink.kinds(&id),
        vec!["stt:partial", "llm:delta", "llm:done"]
    );
    match &h.sink.for_session(&id)[0] {
        CoreEvent::SttPartial { text, is_final, .. } => {
            assert_eq!(text, "What is Rust?");
            assert!(is_final);
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(done.transcript, "What is Rust?");
    assert_eq!(done.metrics.audio_drain_ms, 0);
    assert_eq!(done.metrics.stt_finalize_ms, 0);
    assert_eq!(done.metrics.first_token_ms, 300);
    assert_eq!(done.metrics.total_ms, 300);
    // No audio, no STT for an ask; only the provider key is read.
    assert!(h.audio.calls().is_empty());
    assert_eq!(h.stt.connects(), 0);
    assert_eq!(h.provider.prewarms(), 1);
}

#[tokio::test(start_paused = true)]
async fn ask_without_deepgram_key_still_works() {
    let h = Harness::new();
    h.settings.set_secret("deepgram", None);
    let id = h.handle.ask("Hello?".into()).await.unwrap();
    h.sink.wait_done(&id).await;
}

#[tokio::test(start_paused = true)]
async fn ask_while_recording_supersedes_silently() {
    let h = Harness::new();
    let s1 = h.start_recording().await;
    assert!(h.audio.push_frame(1));
    let s2 = h.handle.ask("Typed question".into()).await.unwrap();
    assert_eq!(s2, SessionId("s2".into()));
    h.sink.wait_done(&s2).await;
    tokio::time::sleep(secs(300)).await;
    // s1 got no terminal event and nothing after the supersede.
    assert!(!h.sink.has_terminal(&s1));
    assert!(!h
        .sink
        .kinds(&s1)
        .iter()
        .any(|k| *k == "session:error" || *k == "llm:done"));
    // Discard-stop, never drain; the socket was aborted, never CloseStream.
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Discard]);
    assert!(!h.stt.log(0).contains(&SttLog::Close));
    assert!(h.stt.log(0).contains(&SttLog::Abort));
}

#[tokio::test(start_paused = true)]
async fn empty_ask_is_rejected_and_leaves_live_session_untouched() {
    let h = Harness::new();
    let s1 = h.start_recording().await;
    let reads = h.settings.reads();
    for text in ["", "   ", "\n\t "] {
        let err = h.handle.ask(text.into()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.message, copy::EMPTY_QUESTION);
    }
    assert_eq!(h.settings.reads(), reads, "no key read for invalid input");
    assert_eq!(h.handle.status().id, Some(s1.clone()));
    assert_eq!(h.handle.status().phase, Phase::Recording);
    assert!(h.audio.push_frame(3));
    h.handle
        .stop_session(&s1)
        .await
        .expect("the live session is still stoppable");
    h.sink.wait_done(&s1).await;
    assert_eq!(h.audio.calls(), vec![AudioCall::Start, AudioCall::Drain]);
}

#[tokio::test(start_paused = true)]
async fn missing_keys_error_without_touching_the_live_session() {
    let h = Harness::new();
    let s1 = h.start_recording().await;

    h.settings.set_secret("deepgram", None);
    let err = h.handle.start_session().await.unwrap_err();
    assert_eq!(
        (err.code, err.message.as_str()),
        (ErrorCode::NoSttKey, copy::NO_STT_KEY)
    );

    h.settings.set_secret("deepgram", Some("   "));
    let err = h.handle.start_session().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NoSttKey, "blank key reads as missing");

    h.settings.set_secret("deepgram", Some(DG_KEY));
    h.settings.set_secret("anthropic", None);
    let err = h.handle.start_session().await.unwrap_err();
    assert_eq!(
        (err.code, err.message.as_str()),
        (ErrorCode::NoLlmKey, copy::NO_LLM_KEY)
    );
    let err = h.handle.ask("q".into()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NoLlmKey);

    h.settings
        .set_read_error(Some("Could not decrypt the key store."));
    let err = h.handle.start_session().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);

    assert_eq!(h.handle.status().id, Some(s1.clone()));
    assert_eq!(h.handle.status().phase, Phase::Recording);
    assert!(h.sink.for_session(&s1).iter().all(|e| !is_terminal(e)));
    assert_eq!(h.audio.calls(), vec![AudioCall::Start]);
}

#[tokio::test(start_paused = true)]
async fn selected_provider_is_used_and_unknown_id_falls_back_to_default() {
    let h = Harness::new();
    h.settings.set_secret("groq", Some("gsk-groq"));
    h.settings.update_config(|c| c.provider_id = "groq".into());
    let id = h.handle.ask("q1".into()).await.unwrap();
    h.sink.wait_done(&id).await;
    assert_eq!(h.groq.stream_calls(), 1);
    assert_eq!(h.groq.keys(), vec!["gsk-groq".to_string()]);
    assert_eq!(h.provider.stream_calls(), 0);

    h.settings.update_config(|c| c.provider_id = "nope".into());
    let id = h.handle.ask("q2".into()).await.unwrap();
    h.sink.wait_done(&id).await;
    assert_eq!(h.provider.stream_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn status_walks_starting_recording_finalizing_answering_idle() {
    let h = Harness::new();
    h.audio.set_start_delay(ms(100));
    h.audio.set_drain(ms(100), vec![]);
    h.provider.push_script(Script::ok_after(ms(500), &["a"]));

    let mut watch = h.handle.status_watch();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![watch.borrow().phase]));
    let seen2 = seen.clone();
    let watcher = tokio::spawn(async move {
        while watch.changed().await.is_ok() {
            let p = watch.borrow_and_update().phase;
            let mut s = seen2.lock().unwrap();
            if s.last() != Some(&p) {
                s.push(p);
            }
        }
    });

    let id = h.handle.start_session().await.unwrap();
    let st = h.handle.status();
    assert_eq!(
        (st.id.clone(), st.phase, st.recording),
        (Some(id.clone()), Phase::Starting, None)
    );

    h.sink.wait_kind(&id, "session:recording").await;
    let st = h.handle.status();
    assert_eq!(st.phase, Phase::Recording);
    let (deadline_ms, cap_ms) = match &h.sink.for_session(&id)[0] {
        CoreEvent::SessionRecording {
            deadline_ms,
            cap_ms,
            ..
        } => (*deadline_ms, *cap_ms),
        e => panic!("{e:?}"),
    };
    // Invariant 11: one deadline for the core cap and the UI countdown.
    let info = st.recording.expect("recording info while recording");
    assert_eq!((info.deadline_ms, info.cap_ms), (deadline_ms, cap_ms));
    assert_eq!(cap_ms, 120_000);
    assert_eq!(deadline_ms, EPOCH_BASE + 100 + 120_000);

    h.wait_until("connected", || h.stt.connected()).await;
    h.handle.stop_session(&id).await.unwrap();
    let st = h.handle.status();
    assert_eq!((st.phase, st.recording), (Phase::Finalizing, None));

    h.sink.wait_kind(&id, "stt:partial").await;
    assert_eq!(h.handle.status().phase, Phase::Answering);
    h.sink.wait_done(&id).await;
    assert_eq!(h.handle.status(), callcore_contract::SessionStatus::idle());
    h.settle().await;
    watcher.abort();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            Phase::Idle,
            Phase::Starting,
            Phase::Recording,
            Phase::Finalizing,
            Phase::Answering,
            Phase::Idle
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn session_ids_are_sequential_and_never_reused() {
    let h = Harness::new();
    let a = h.handle.start_session().await.unwrap();
    let b = h.handle.start_session().await.unwrap();
    let c = h.handle.ask("q".into()).await.unwrap();
    assert_eq!(
        (a.0.as_str(), b.0.as_str(), c.0.as_str()),
        ("s1", "s2", "s3")
    );
    h.sink.wait_done(&c).await;
}

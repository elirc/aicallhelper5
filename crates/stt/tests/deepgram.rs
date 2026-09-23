//! Socket-level tests against a loopback fake Deepgram (real time, bounded
//! waits, no fixed sleeps as synchronization).

mod support;

use std::time::Duration;

use callcore_contract::copy;
use callcore_contract::ports::{
    AudioFrame, SttConnection, SttConnector, SttEvent, SttFailure, SttFailureKind,
};
use callcore_contract::Secret;
use callcore_stt::{DeepgramConnector, CLOSE_STREAM_MESSAGE, KEEPALIVE_MESSAGE};
use support::*;
use tokio_tungstenite::tungstenite::Message;

fn key() -> Secret {
    Secret::new(TEST_KEY)
}

fn frame(seed: i16, len: usize) -> AudioFrame {
    AudioFrame {
        samples: (0..len)
            .map(|i| seed.wrapping_mul(31).wrapping_add(i as i16))
            .collect(),
        rms: 0.25,
    }
}

async fn connected(server: &mut TestServer) -> (SttConnection, ServerConn) {
    let conn = within(DeepgramConnector::with_url(&server.url).connect(&key()))
        .await
        .expect("connect");
    let sc = server.accept().await;
    (conn, sc)
}

fn assert_no_key(s: &str) {
    assert!(!s.contains(TEST_KEY), "key leaked: {s}");
    assert!(!s.contains("SECRET"), "key leaked: {s}");
}

fn failed(ev: Option<SttEvent>) -> SttFailure {
    match ev {
        Some(SttEvent::Failed(f)) => f,
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn handshake_sends_token_subprotocol_and_keeps_key_out_of_url() {
    let mut server = start(Mode::Accept).await;
    let (_conn, _sc) = connected(&mut server).await;
    let hs = server.handshake().await;
    assert_eq!(
        hs.protocol.as_deref(),
        Some(format!("token, {TEST_KEY}").as_str())
    );
    assert_no_key(&hs.uri);
    assert!(hs.uri.starts_with("/v1/listen?model=nova-3"), "{}", hs.uri);
    assert!(!hs.uri.contains("endpointing") && !hs.uri.contains("no_delay"));
    assert!(!hs.all_headers.to_lowercase().contains("authorization"));
}

#[tokio::test]
async fn audio_frames_arrive_byte_exact_in_order() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    let mut frames: Vec<AudioFrame> = (1..=12).map(|i| frame(i, 2048)).collect();
    frames.push(frame(99, 777)); // final partial frame from a drain
    for f in &frames {
        conn.sender.send_audio(f).await.expect("send");
    }
    for f in &frames {
        match sc.recv_non_keepalive().await {
            Some(Message::Binary(b)) => assert_eq!(b, f.to_le_bytes()),
            other => panic!("expected binary, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn keepalive_sent_while_idle() {
    let mut server = start(Mode::Accept).await;
    let connector =
        DeepgramConnector::with_url_and_keepalive(&server.url, Duration::from_millis(100));
    let _conn = within(connector.connect(&key())).await.expect("connect");
    let mut sc = server.accept().await;
    for _ in 0..3 {
        match sc.recv().await {
            Some(Message::Text(t)) => assert_eq!(t, KEEPALIVE_MESSAGE),
            other => panic!("expected KeepAlive, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn close_stream_sent_after_all_audio_and_nothing_after_it() {
    let mut server = start(Mode::Accept).await;
    let connector =
        DeepgramConnector::with_url_and_keepalive(&server.url, Duration::from_millis(50));
    let mut conn = within(connector.connect(&key())).await.expect("connect");
    let mut sc = server.accept().await;
    let frames: Vec<AudioFrame> = (1..=5).map(|i| frame(i, 2048)).collect();
    for f in &frames {
        conn.sender.send_audio(f).await.unwrap();
    }
    conn.sender.close_stream().await.unwrap();
    for f in &frames {
        match sc.recv_non_keepalive().await {
            Some(Message::Binary(b)) => assert_eq!(b, f.to_le_bytes()),
            other => panic!("expected binary, got {other:?}"),
        }
    }
    match sc.recv_non_keepalive().await {
        Some(Message::Text(t)) => assert_eq!(t, CLOSE_STREAM_MESSAGE),
        other => panic!("expected CloseStream, got {other:?}"),
    }
    // Absence check (not synchronization): with a 50 ms keepalive, 8 intervals
    // pass without the client sending anything more after CloseStream.
    let quiet = tokio::time::timeout(
        Duration::from_millis(400),
        futures_util::StreamExt::next(&mut sc.ws),
    )
    .await;
    assert!(quiet.is_err(), "client sent after CloseStream: {quiet:?}");
    // Sending audio after CloseStream is refused locally.
    let err = conn.sender.send_audio(&frames[0]).await.unwrap_err();
    assert_eq!(err.kind, SttFailureKind::Closed);
}

#[tokio::test]
async fn scripted_results_produce_transcripts_and_flushed() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.send_text(results("hel", false)).await;
    sc.send_text(results("hello wor", false)).await;
    sc.send_text(results("hello world", true)).await;
    sc.send_text(results("how", false)).await;

    let expect = [
        ("hel", false),
        ("hello wor", false),
        ("hello world", true),
        ("hello world how", false),
    ];
    for (text, is_final) in expect {
        assert_eq!(
            next_event(&mut conn.events).await,
            Some(SttEvent::Transcript {
                text: text.into(),
                is_final
            })
        );
    }

    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    sc.send_text(results("how are you", true)).await;
    sc.send_text(metadata()).await;
    sc.close(1000, "").await;

    let rest = drain_events(&mut conn.events).await;
    assert_eq!(
        rest,
        vec![
            SttEvent::Transcript {
                text: "hello world how are you".into(),
                is_final: true
            },
            SttEvent::Flushed {
                transcript: "hello world how are you".into()
            },
        ]
    );
}

#[tokio::test]
async fn flushed_includes_trailing_interim_not_replaced_by_a_final() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.send_text(results("done", true)).await;
    sc.send_text(results("dangling", false)).await;
    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    sc.close(1000, "").await;
    let events = drain_events(&mut conn.events).await;
    assert_eq!(
        events.last(),
        Some(&SttEvent::Flushed {
            transcript: "done dangling".into()
        })
    );
}

#[tokio::test]
async fn flushed_with_no_speech_is_empty() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    sc.send_text(metadata()).await;
    sc.close(1000, "").await;
    assert_eq!(
        drain_events(&mut conn.events).await,
        vec![SttEvent::Flushed {
            transcript: String::new()
        }]
    );
}

#[tokio::test]
async fn hostile_frames_are_ignored_without_crashing() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.send_text("not json at all {{{").await;
    sc.send_text(r#"{"channel":5}"#).await;
    sc.send_text(
        r#"{"type":"Results","is_final":"true","channel":{"alternatives":[{"transcript":"x"}]}}"#,
    )
    .await;
    sc.send_text(r#"{"type":"Results","is_final":true,"channel":{"alternatives":[]}}"#)
        .await;
    sc.send_text(r#"{"type":"SpeechStarted","timestamp":0.1}"#)
        .await;
    sc.send_text(r#"{"type":"UtteranceEnd","last_word_end":1.0}"#)
        .await;
    sc.send_text(metadata()).await;
    // 1.5 MiB: over the parse cap, under tungstenite's message cap -> ignored.
    let huge = results(&"a".repeat(1_500_000), true);
    sc.send_text(huge).await;
    sc.send(Message::Binary(vec![0xFF; 4096])).await;
    sc.send_text("[".repeat(10_000)).await;
    sc.send_text(results("survivor", true)).await;

    // The first (and only) event is the valid frame sent last.
    assert_eq!(
        next_event(&mut conn.events).await,
        Some(SttEvent::Transcript {
            text: "survivor".into(),
            is_final: true
        })
    );
    // The connection is still usable.
    conn.sender.send_audio(&frame(1, 2048)).await.unwrap();
    assert!(matches!(
        sc.recv_non_keepalive().await,
        Some(Message::Binary(_))
    ));
}

#[tokio::test]
async fn frame_over_tungstenite_cap_fails_as_server_error_not_panic() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    let _ = futures_util::SinkExt::send(&mut sc.ws, Message::Text("x".repeat(5 << 20))).await;
    let f = failed(next_event(&mut conn.events).await);
    assert_eq!(f.kind, SttFailureKind::Server);
    assert_no_key(&f.message);
    assert_eq!(
        next_event(&mut conn.events).await,
        None,
        "nothing after terminal"
    );
}

#[tokio::test]
async fn close_1008_maps_to_bad_key_with_code() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.close(1008, "Invalid credentials\r\n").await;
    let f = failed(next_event(&mut conn.events).await);
    assert_eq!(f.kind, SttFailureKind::BadKey);
    assert_eq!(
        f.message,
        "Deepgram closed the connection (code 1008: Invalid credentials)"
    );
    assert_eq!(next_event(&mut conn.events).await, None);
    let err = conn.sender.send_audio(&frame(1, 10)).await.unwrap_err();
    assert_eq!(err.kind, SttFailureKind::Closed);
}

#[tokio::test]
async fn other_close_code_is_server_failure_with_code() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.send_text(results("partial", false)).await;
    sc.close(1011, "internal").await;
    assert!(matches!(
        next_event(&mut conn.events).await,
        Some(SttEvent::Transcript { .. })
    ));
    let f = failed(next_event(&mut conn.events).await);
    assert_eq!(f.kind, SttFailureKind::Server);
    assert_eq!(
        f.message,
        "Deepgram closed the connection (code 1011: internal)"
    );
}

#[tokio::test]
async fn bad_close_after_close_stream_is_still_a_failure() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    sc.close(1011, "").await;
    let events = drain_events(&mut conn.events).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        failed(events.into_iter().next()).kind,
        SttFailureKind::Server
    );
}

#[tokio::test]
async fn normal_close_before_close_stream_is_unexpected_end() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.close(1000, "").await;
    let f = failed(next_event(&mut conn.events).await);
    assert_eq!(f.kind, SttFailureKind::Server);
    assert_eq!(f.message, "Deepgram ended the stream unexpectedly");
}

#[tokio::test]
async fn abrupt_tcp_drop_mid_stream_is_server_failure_then_send_is_closed() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, sc) = connected(&mut server).await;
    conn.sender.send_audio(&frame(1, 2048)).await.unwrap();
    drop(sc); // TCP goes away without a close frame
    let f = failed(next_event(&mut conn.events).await);
    assert_eq!(f.kind, SttFailureKind::Server);
    assert!(
        f.message.starts_with("Lost the connection to Deepgram"),
        "{}",
        f.message
    );
    assert_eq!(next_event(&mut conn.events).await, None);
    let err = conn.sender.send_audio(&frame(2, 2048)).await.unwrap_err();
    assert_eq!(err.kind, SttFailureKind::Closed);
    let err = conn.sender.close_stream().await.unwrap_err();
    assert_eq!(err.kind, SttFailureKind::Closed);
}

#[tokio::test]
async fn late_close_after_flushed_emits_no_failed() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    sc.send_text(results("final words", true)).await;
    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    sc.close(1000, "").await;
    // Then the server rudely drops the TCP connection and tries more frames.
    let _ = futures_util::SinkExt::send(&mut sc.ws, Message::Text(results("ghost", true))).await;
    drop(sc);
    let events = drain_events(&mut conn.events).await;
    assert_eq!(
        events,
        vec![
            SttEvent::Transcript {
                text: "final words".into(),
                is_final: true
            },
            SttEvent::Flushed {
                transcript: "final words".into()
            },
        ]
    );
    // Keeping the sender around (or aborting it) after Flushed never errors.
    conn.sender.abort();
}

#[tokio::test]
async fn handshake_401_and_403_are_bad_key_without_the_key() {
    for status in [401u16, 403] {
        let mut server = start(Mode::Reject(status)).await;
        let err = within(DeepgramConnector::with_url(&server.url).connect(&key()))
            .await
            .err()
            .expect("must fail");
        assert_eq!(err.kind, SttFailureKind::BadKey, "{status}");
        assert!(err.message.contains(&status.to_string()), "{}", err.message);
        assert_no_key(&err.message);
        assert_no_key(&format!("{err:?}"));
        // The key still went only in the subprotocol header.
        let hs = server.handshake().await;
        assert_no_key(&hs.uri);
    }
}

#[tokio::test]
async fn handshake_500_is_connect_with_user_copy() {
    let server = start(Mode::Reject(500)).await;
    let err = within(DeepgramConnector::with_url(&server.url).connect(&key()))
        .await
        .err()
        .expect("must fail");
    assert_eq!(err.kind, SttFailureKind::Connect);
    assert_eq!(err.message, copy::STT_CONNECT);
}

#[tokio::test]
async fn connection_refused_is_connect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let url = format!("ws://127.0.0.1:{port}/v1/listen");
    let err = within(DeepgramConnector::with_url(&url).connect(&key()))
        .await
        .err()
        .expect("must fail");
    assert_eq!(err.kind, SttFailureKind::Connect);
    assert_eq!(err.message, copy::STT_CONNECT);
    assert_no_key(&format!("{err:?}"));
}

#[tokio::test]
async fn invalid_url_is_connect_not_panic() {
    let err = DeepgramConnector::with_url("not a url")
        .connect(&key())
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, SttFailureKind::Connect);
}

#[tokio::test]
async fn abort_stops_tasks_and_server_sees_eof() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    conn.sender.send_audio(&frame(1, 2048)).await.unwrap();
    assert!(matches!(
        sc.recv_non_keepalive().await,
        Some(Message::Binary(_))
    ));
    conn.sender.abort();
    conn.sender.abort(); // idempotent
    sc.wait_client_gone().await;
    // The reader task is gone, so the event channel ends without a Failed.
    assert_eq!(drain_events(&mut conn.events).await, Vec::<SttEvent>::new());
    let err = conn.sender.send_audio(&frame(2, 2048)).await.unwrap_err();
    assert_eq!(err.kind, SttFailureKind::Closed);
    assert_eq!(
        conn.sender.close_stream().await.unwrap_err().kind,
        SttFailureKind::Closed
    );
}

#[tokio::test]
async fn abort_after_close_stream_also_tears_down() {
    let mut server = start(Mode::Accept).await;
    let (mut conn, mut sc) = connected(&mut server).await;
    conn.sender.close_stream().await.unwrap();
    assert_eq!(
        sc.recv_non_keepalive().await,
        Some(Message::Text(CLOSE_STREAM_MESSAGE.into()))
    );
    conn.sender.abort();
    sc.wait_client_gone().await;
    assert_eq!(drain_events(&mut conn.events).await, Vec::<SttEvent>::new());
}

#[tokio::test]
async fn dropping_sender_before_close_stream_tears_down() {
    let mut server = start(Mode::Accept).await;
    let (conn, mut sc) = connected(&mut server).await;
    let SttConnection { sender, events } = conn;
    drop(sender);
    sc.wait_client_gone().await;
    drop(events);
}

#[tokio::test]
async fn dropping_event_receiver_stops_the_reader() {
    let mut server = start(Mode::Accept).await;
    let (conn, mut sc) = connected(&mut server).await;
    let SttConnection { mut sender, events } = conn;
    sender.close_stream().await.unwrap();
    drop(events);
    drop(sender);
    // Reader notices the closed channel and drops the socket.
    sc.wait_client_gone().await;
}

#[tokio::test]
async fn debug_output_never_contains_the_key() {
    let mut server = start(Mode::Accept).await;
    let connector = DeepgramConnector::with_url(&server.url);
    let conn = within(connector.connect(&key())).await.unwrap();
    let _sc = server.accept().await;
    let s = format!("{connector:?} {:?}", key());
    assert_no_key(&s);
    let dg = callcore_stt::DeepgramConnector::new();
    assert_no_key(&format!("{dg:?}"));
    // Every failure the crate can produce is key-free (Display and Debug).
    let refused = DeepgramConnector::with_url("ws://127.0.0.1:1/x")
        .connect(&key())
        .await
        .err();
    assert_no_key(&format!("{refused:?}"));
    drop(conn);
}

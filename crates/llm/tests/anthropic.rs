//! Anthropic provider conformance matrix against a loopback HTTP server.

mod support;

use callcore_contract::ports::{
    AnswerProvider, PreparedRequest, PromptParts, ProviderFailure, ProviderFailureKind,
    ProviderRegistry, StreamOutcome,
};
use callcore_contract::{Finish, Secret};
use callcore_llm::{http_client, Registry};
use serde_json::json;
use support::{dead_port, drain, hostile, Framing, Reply, Script, TestServer};
use tokio::sync::mpsc;

const KEY: &str = "sk-ant-TEST-KEY-do-not-leak";

fn prompt() -> PromptParts {
    PromptParts {
        cached_prefix: "You are me. Résumé: …".into(),
        style_suffix: "Answer briefly.".into(),
        user_message: "Tell me about yourself?".into(),
    }
}

fn provider(origin: &str) -> std::sync::Arc<dyn AnswerProvider> {
    Registry::with_origins(http_client(), origin, "http://127.0.0.1:9")
        .get("anthropic")
        .unwrap()
}

fn event(ty: &str, data: serde_json::Value) -> String {
    format!("event: {ty}\ndata: {data}\n\n")
}

fn start() -> String {
    event(
        "message_start",
        json!({"type":"message_start","message":{"id":"msg_1","model":"claude-haiku-4-5","content":[]}}),
    )
}

fn delta(index: u32, text: &str) -> String {
    event(
        "content_block_delta",
        json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
    )
}

fn block_start(index: u32) -> String {
    event(
        "content_block_start",
        json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
    )
}

fn block_stop(index: u32) -> String {
    event(
        "content_block_stop",
        json!({"type":"content_block_stop","index":index}),
    )
}

fn stop_reason(r: &str) -> String {
    event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":r},"usage":{"output_tokens":5}}),
    )
}

fn msg_stop() -> String {
    event("message_stop", json!({"type":"message_stop"}))
}

fn ping() -> String {
    event("ping", json!({"type":"ping"}))
}

fn error_event(ty: &str, msg: &str) -> String {
    event(
        "error",
        json!({"type":"error","error":{"type":ty,"message":msg}}),
    )
}

/// A full successful transcript with two content blocks.
fn success_body(stop: &str) -> String {
    [
        start(),
        block_start(0),
        ping(),
        delta(0, "Hi — I'm "),
        delta(0, "Eli, naïve café 日本語 🚀."),
        block_stop(0),
        block_start(1),
        delta(1, " Second block."),
        block_stop(1),
        stop_reason(stop),
        msg_stop(),
    ]
    .concat()
}

async fn run(
    reply: Reply,
) -> (
    Result<StreamOutcome, ProviderFailure>,
    Vec<String>,
    TestServer,
    PreparedRequest,
) {
    run_script(Script::Reply(reply)).await
}

async fn run_script(
    script: Script,
) -> (
    Result<StreamOutcome, ProviderFailure>,
    Vec<String>,
    TestServer,
    PreparedRequest,
) {
    let server = TestServer::start(vec![script]).await;
    let p = provider(&server.origin());
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let res = p.stream(&req, tx).await;
    let deltas = drain(&mut rx);
    (res, deltas, server, req)
}

fn kind(res: &Result<StreamOutcome, ProviderFailure>) -> ProviderFailureKind {
    res.as_ref().expect_err("expected a failure").kind
}

fn assert_no_key(res: &Result<StreamOutcome, ProviderFailure>) {
    if let Err(f) = res {
        assert!(!f.message.contains("TEST-KEY"), "key leaked: {}", f.message);
        assert!(!format!("{f:?}").contains("TEST-KEY"));
    }
}

#[tokio::test]
async fn anthropic_success_hostile_chunking_concatenates_blocks() {
    let body = success_body("end_turn");
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    let out = res.unwrap();
    assert_eq!(out.finish, Finish::Complete);
    assert_eq!(
        out.answer,
        "Hi — I'm Eli, naïve café 日本語 🚀. Second block."
    );
    assert_eq!(deltas.concat(), out.answer);
    assert_eq!(
        deltas,
        vec!["Hi — I'm ", "Eli, naïve café 日本語 🚀.", " Second block."]
    );
}

#[tokio::test]
async fn anthropic_success_content_length_and_close_delimited_framing() {
    for framing in [Framing::ContentLength, Framing::CloseDelimited] {
        let body = success_body("end_turn");
        let (res, _, _s, _) = run(Reply::sse(hostile(body.as_bytes())).framing(framing)).await;
        assert_eq!(res.unwrap().finish, Finish::Complete, "{framing:?}");
    }
}

#[tokio::test]
async fn anthropic_crlf_line_endings_parse() {
    let body = success_body("end_turn").replace('\n', "\r\n");
    let (res, _, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(
        res.unwrap().answer,
        "Hi — I'm Eli, naïve café 日本語 🚀. Second block."
    );
}

#[tokio::test]
async fn anthropic_message_stop_in_unterminated_final_line_is_flushed() {
    let body = [start(), delta(0, "Hello"), stop_reason("end_turn")].concat()
        + "data: {\"type\":\"message_stop\"}";
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(res.unwrap().answer, "Hello");
}

#[tokio::test]
async fn anthropic_error_event_before_text_is_provider_error() {
    let body = [start(), error_event("api_error", "Internal server error")].concat();
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(kind(&res), ProviderFailureKind::ProviderError);
    assert!(res.unwrap_err().message.contains("Internal server error"));
    assert!(deltas.is_empty());
}

#[tokio::test]
async fn anthropic_error_event_after_text_is_provider_error_with_deltas_sent() {
    let body = [
        start(),
        delta(0, "Partial "),
        error_event("api_error", "boom"),
    ]
    .concat();
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(kind(&res), ProviderFailureKind::ProviderError);
    assert_eq!(deltas, vec!["Partial "]);
}

#[tokio::test]
async fn anthropic_overloaded_and_rate_limit_events_map_to_rate_limit() {
    for (ty, status) in [("overloaded_error", 529), ("rate_limit_error", 429)] {
        let body = [start(), error_event(ty, "Overloaded")].concat();
        let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
        assert_eq!(
            kind(&res),
            ProviderFailureKind::RateLimit { status },
            "{ty}"
        );
        assert!(res.unwrap_err().message.contains(&format!("({status})")));
    }
}

#[tokio::test]
async fn anthropic_empty_answer_is_empty_answer() {
    let body = [
        start(),
        block_start(0),
        block_stop(0),
        stop_reason("end_turn"),
        msg_stop(),
    ]
    .concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::EmptyAnswer);
}

#[tokio::test]
async fn anthropic_whitespace_answer_is_empty_answer() {
    let body = [
        start(),
        delta(0, "  \n\t "),
        stop_reason("end_turn"),
        msg_stop(),
    ]
    .concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::EmptyAnswer);
}

#[tokio::test]
async fn anthropic_premature_eof_is_incomplete() {
    let body = [start(), delta(0, "Half an ans"), stop_reason("end_turn")].concat();
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(kind(&res), ProviderFailureKind::Incomplete);
    assert_eq!(deltas, vec!["Half an ans"]);
}

#[tokio::test]
async fn anthropic_pings_only_is_incomplete() {
    let body = [ping(), ping(), ping()].concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::Incomplete);
}

#[tokio::test]
async fn anthropic_token_limit_is_truncated() {
    for r in ["max_tokens", "model_context_window_exceeded"] {
        let (res, _, _s, _) = run(Reply::sse(vec![success_body(r).into_bytes()])).await;
        assert_eq!(res.unwrap().finish, Finish::Truncated, "{r}");
    }
}

#[tokio::test]
async fn anthropic_refusal_is_refused() {
    let (res, _, _s, _) = run(Reply::sse(vec![success_body("refusal").into_bytes()])).await;
    assert_eq!(res.unwrap().finish, Finish::Refused);
}

#[tokio::test]
async fn anthropic_other_stop_reasons_are_complete() {
    for r in ["end_turn", "stop_sequence", "pause_turn"] {
        let (res, _, _s, _) = run(Reply::sse(vec![success_body(r).into_bytes()])).await;
        assert_eq!(res.unwrap().finish, Finish::Complete, "{r}");
    }
}

#[tokio::test]
async fn anthropic_http_status_matrix() {
    let err =
        |ty: &str, m: &str| json!({"type":"error","error":{"type":ty,"message":m}}).to_string();
    let cases = [
        (
            401,
            err("authentication_error", "invalid x-api-key"),
            ProviderFailureKind::Auth { status: 401 },
        ),
        (
            403,
            err("permission_error", "nope"),
            ProviderFailureKind::Auth { status: 403 },
        ),
        (
            404,
            err("not_found_error", "model: claude-haiku-4-5"),
            ProviderFailureKind::ModelUnavailable,
        ),
        (
            429,
            err("rate_limit_error", "slow down"),
            ProviderFailureKind::RateLimit { status: 429 },
        ),
        (
            500,
            err("api_error", "Internal server error"),
            ProviderFailureKind::Http { status: 500 },
        ),
        (
            529,
            err("overloaded_error", "Overloaded"),
            ProviderFailureKind::RateLimit { status: 529 },
        ),
    ];
    for (status, body, expected) in cases {
        let (res, deltas, _s, _) = run(Reply::status(status, &body)).await;
        assert_eq!(kind(&res), expected, "status {status}");
        assert!(deltas.is_empty());
        assert_no_key(&res);
        let msg = res.unwrap_err().message;
        if expected != ProviderFailureKind::ModelUnavailable {
            assert!(
                msg.contains(&format!("({status})")),
                "status not quoted: {msg}"
            );
        }
        match status {
            401 => assert!(
                msg.contains("Anthropic rejected the API key (401)"),
                "{msg}"
            ),
            404 => assert!(
                msg.contains("claude-haiku-4-5") && msg.contains("Settings"),
                "{msg}"
            ),
            500 => assert_eq!(
                msg,
                "Anthropic returned an error (500): Internal server error"
            ),
            _ => {}
        }
    }
}

#[tokio::test]
async fn anthropic_key_echoed_in_error_body_is_redacted() {
    let body = json!({"type":"error","error":{"type":"invalid_request_error","message":format!("bad header x-api-key: {KEY}")}}).to_string();
    let (res, _, _s, _) = run(Reply::status(400, &body)).await;
    assert_eq!(kind(&res), ProviderFailureKind::Http { status: 400 });
    assert_no_key(&res);
    assert!(res.unwrap_err().message.contains("x-api-key: ***"));
    // Also for an in-stream error event.
    let ev = [start(), error_event("api_error", &format!("echo {KEY}"))].concat();
    let (res, _, _s, _) = run(Reply::sse(vec![ev.into_bytes()])).await;
    assert_no_key(&res);
}

#[tokio::test]
async fn anthropic_long_error_body_snippet_is_capped() {
    let long = "e".repeat(5000);
    let (res, _, _s, _) = run(Reply::status(500, &long)).await;
    let msg = res.unwrap_err().message;
    let snippet = msg.split_once(": ").unwrap().1;
    assert!(snippet.chars().count() <= 200, "{}", snippet.len());
}

#[tokio::test]
async fn anthropic_stream_drop_mid_body_is_stream_drop() {
    for framing in [Framing::Chunked, Framing::ContentLength] {
        let body = [start(), delta(0, "Some text")].concat();
        let reply = Reply::sse(hostile(body.as_bytes()))
            .framing(framing)
            .truncated();
        let (res, deltas, _s, _) = run(reply).await;
        assert_eq!(kind(&res), ProviderFailureKind::StreamDrop, "{framing:?}");
        assert_eq!(deltas, vec!["Some text"]);
    }
}

#[tokio::test]
async fn anthropic_close_before_response_is_connect() {
    let (res, _, _s, _) = run_script(Script::CloseWithoutResponse).await;
    assert_eq!(kind(&res), ProviderFailureKind::Connect);
}

#[tokio::test]
async fn anthropic_connection_refused_is_connect() {
    let p = provider(&format!("http://127.0.0.1:{}", dead_port()));
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, _rx) = mpsc::unbounded_channel();
    let res = p.stream(&req, tx).await;
    assert_eq!(kind(&res), ProviderFailureKind::Connect);
    assert_no_key(&res);
    let msg = res.unwrap_err().message;
    assert!(msg.starts_with("Could not connect to Anthropic"), "{msg}");
}

#[tokio::test]
async fn anthropic_dropped_receiver_is_aborted() {
    let server = TestServer::start(vec![Script::Reply(Reply::sse(vec![success_body(
        "end_turn",
    )
    .into_bytes()]))])
    .await;
    let p = provider(&server.origin());
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, rx) = mpsc::unbounded_channel();
    drop(rx);
    assert_eq!(
        kind(&p.stream(&req, tx).await),
        ProviderFailureKind::Aborted
    );
}

#[tokio::test]
async fn anthropic_request_bytes_identical_across_two_sends() {
    let body = success_body("end_turn").into_bytes();
    let server = TestServer::start(vec![
        Script::Reply(Reply::status(500, "{}")),
        Script::Reply(Reply::sse(vec![body])),
    ])
    .await;
    let p = provider(&server.origin());
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, _rx) = mpsc::unbounded_channel();
    assert!(p.stream(&req, tx.clone()).await.is_err());
    assert!(p.stream(&req, tx).await.is_ok());
    let recs = server.recorded();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].body, recs[1].body);
    assert_eq!(recs[0].body, req.body.to_vec());
    assert_eq!(recs[0].path, recs[1].path);
    // Building again from the same inputs is byte-identical too.
    assert_eq!(p.build_request(&prompt(), &Secret::new(KEY)).body, req.body);
}

#[tokio::test]
async fn anthropic_request_body_is_exactly_spec_section_7() {
    let (_, _, server, _) = run(Reply::sse(vec![success_body("end_turn").into_bytes()])).await;
    let rec = &server.recorded()[0];
    let v: serde_json::Value = serde_json::from_slice(&rec.body).unwrap();
    assert_eq!(
        v,
        json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "stream": true,
            "system": [
                {"type":"text","text":"You are me. Résumé: …","cache_control":{"type":"ephemeral"}},
                {"type":"text","text":"Answer briefly."}
            ],
            "messages": [{"role":"user","content":"Tell me about yourself?"}]
        })
    );
}

#[tokio::test]
async fn anthropic_headers_present_and_correct() {
    let (_, _, server, _) = run(Reply::sse(vec![success_body("end_turn").into_bytes()])).await;
    let rec = &server.recorded()[0];
    assert_eq!(rec.method, "POST");
    assert_eq!(rec.path, "/v1/messages");
    assert_eq!(rec.header("x-api-key"), Some(KEY));
    assert_eq!(rec.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(rec.header("content-type"), Some("application/json"));
    assert!(rec.header("authorization").is_none());
}

#[test]
fn anthropic_prepared_request_debug_never_shows_key() {
    let p = provider("http://127.0.0.1:9");
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let dbg = format!("{req:?}");
    assert!(!dbg.contains("TEST-KEY"), "{dbg}");
    assert!(dbg.contains("***"));
    assert_eq!(req.url, "http://127.0.0.1:9/v1/messages");
}

#[tokio::test]
async fn anthropic_prewarm_gets_models_without_key() {
    let server = TestServer::start(vec![Script::Reply(Reply::status(401, "{}"))]).await;
    let p = provider(&server.origin());
    p.prewarm();
    let rec = server.next_request().await;
    assert_eq!(rec.method, "GET");
    assert_eq!(rec.path, "/v1/models");
    assert!(rec.header("x-api-key").is_none());
    assert!(rec.header("authorization").is_none());
}

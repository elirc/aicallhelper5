//! Groq provider conformance matrix against a loopback HTTP server.

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

const KEY: &str = "gsk_TEST-KEY-do-not-leak";
const GONE: &str = "The model openai/gpt-oss-120b is no longer available — switch the answer provider to Claude in Settings or install the latest version.";

fn prompt() -> PromptParts {
    PromptParts {
        cached_prefix: "You are me. Résumé: …".into(),
        style_suffix: "Answer briefly.".into(),
        user_message: "Tell me about yourself?".into(),
    }
}

fn provider(origin: &str) -> std::sync::Arc<dyn AnswerProvider> {
    Registry::with_origins(http_client(), "http://127.0.0.1:9", origin)
        .get("groq")
        .unwrap()
}

fn chunk(content: Option<&str>, finish: Option<&str>) -> String {
    let delta = match content {
        Some(c) => json!({"role":"assistant","content":c}),
        None => json!({}),
    };
    let v = json!({
        "id":"chatcmpl-1","object":"chat.completion.chunk","model":"openai/gpt-oss-120b",
        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]
    });
    format!("data: {v}\n\n")
}

const DONE: &str = "data: [DONE]\n\n";

fn success_body(finish: &str) -> String {
    [
        chunk(Some(""), None),
        chunk(Some("Hi — I'm "), None),
        chunk(Some("Eli, naïve café 日本語 🚀."), None),
        chunk(None, Some(finish)),
        DONE.to_owned(),
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
async fn groq_success_hostile_chunking() {
    let body = success_body("stop");
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    let out = res.unwrap();
    assert_eq!(out.finish, Finish::Complete);
    assert_eq!(out.answer, "Hi — I'm Eli, naïve café 日本語 🚀.");
    assert_eq!(deltas, vec!["Hi — I'm ", "Eli, naïve café 日本語 🚀."]);
}

#[tokio::test]
async fn groq_success_content_length_crlf() {
    let body = success_body("stop").replace('\n', "\r\n");
    let reply = Reply::sse(hostile(body.as_bytes())).framing(Framing::ContentLength);
    let (res, _, _s, _) = run(reply).await;
    assert_eq!(res.unwrap().answer, "Hi — I'm Eli, naïve café 日本語 🚀.");
}

#[tokio::test]
async fn groq_done_alone_is_terminal() {
    let body = [chunk(Some("Answer"), None), DONE.to_owned()].concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    let out = res.unwrap();
    assert_eq!(
        (out.finish, out.answer.as_str()),
        (Finish::Complete, "Answer")
    );
}

#[tokio::test]
async fn groq_bytes_after_done_in_same_chunk_still_count() {
    let body = [
        chunk(Some("Before "), None),
        DONE.to_owned(),
        chunk(Some("after"), None),
    ]
    .concat();
    // Single write, and a server that never closes the body after it would
    // still be fine: the provider stops after the chunk holding [DONE].
    let (res, deltas, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(res.unwrap().answer, "Before after");
    assert_eq!(deltas, vec!["Before ", "after"]);
}

#[tokio::test]
async fn groq_finish_reason_without_done_is_terminal() {
    let body = [chunk(Some("Answer"), None), chunk(None, Some("stop"))].concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(res.unwrap().finish, Finish::Complete);
}

#[tokio::test]
async fn groq_token_limit_is_truncated() {
    let (res, _, _s, _) = run(Reply::sse(vec![success_body("length").into_bytes()])).await;
    assert_eq!(res.unwrap().finish, Finish::Truncated);
}

#[tokio::test]
async fn groq_content_filter_is_refused() {
    let (res, _, _s, _) = run(Reply::sse(
        vec![success_body("content_filter").into_bytes()],
    ))
    .await;
    assert_eq!(res.unwrap().finish, Finish::Refused);
}

#[tokio::test]
async fn groq_error_payload_before_text_is_provider_error() {
    let body = format!(
        "data: {}\n\n",
        json!({"error":{"message":"Service unavailable","type":"internal_server_error"}})
    );
    let (res, deltas, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::ProviderError);
    assert!(res.unwrap_err().message.contains("Service unavailable"));
    assert!(deltas.is_empty());
}

#[tokio::test]
async fn groq_error_payload_after_text_is_provider_error_with_deltas_sent() {
    let body = chunk(Some("Partial "), None)
        + &format!(
            "data: {}\n\n",
            json!({"error":{"message":format!("oops {KEY}")}})
        );
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(kind(&res), ProviderFailureKind::ProviderError);
    assert_eq!(deltas, vec!["Partial "]);
    assert_no_key(&res);
}

#[tokio::test]
async fn groq_empty_answer_is_empty_answer() {
    let body = [
        chunk(Some(""), None),
        chunk(None, Some("stop")),
        DONE.to_owned(),
    ]
    .concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::EmptyAnswer);
}

#[tokio::test]
async fn groq_whitespace_answer_is_empty_answer() {
    let body = [
        chunk(Some(" \n "), None),
        chunk(None, Some("stop")),
        DONE.to_owned(),
    ]
    .concat();
    let (res, _, _s, _) = run(Reply::sse(vec![body.into_bytes()])).await;
    assert_eq!(kind(&res), ProviderFailureKind::EmptyAnswer);
}

#[tokio::test]
async fn groq_premature_eof_flushes_unterminated_line_then_incomplete() {
    let tail = chunk(Some("tail ✓"), None);
    let body = chunk(Some("head "), None) + tail.trim_end();
    let (res, deltas, _s, _) = run(Reply::sse(hostile(body.as_bytes()))).await;
    assert_eq!(kind(&res), ProviderFailureKind::Incomplete);
    assert_eq!(deltas, vec!["head ", "tail ✓"]);
}

#[tokio::test]
async fn groq_http_status_matrix() {
    let err = |m: &str| json!({"error":{"message":m,"type":"invalid_request_error"}}).to_string();
    let cases = [
        (
            401,
            err("Invalid API Key"),
            ProviderFailureKind::Auth { status: 401 },
        ),
        (
            403,
            err("Forbidden"),
            ProviderFailureKind::Auth { status: 403 },
        ),
        (404, err("Not found"), ProviderFailureKind::ModelUnavailable),
        (
            429,
            err("Rate limit reached"),
            ProviderFailureKind::RateLimit { status: 429 },
        ),
        (
            500,
            err("Internal"),
            ProviderFailureKind::Http { status: 500 },
        ),
        (
            529,
            err("Overloaded"),
            ProviderFailureKind::RateLimit { status: 529 },
        ),
    ];
    for (status, body, expected) in cases {
        let (res, deltas, _s, _) = run(Reply::status(status, &body)).await;
        assert_eq!(kind(&res), expected, "status {status}");
        assert!(deltas.is_empty());
        assert_no_key(&res);
        let msg = res.unwrap_err().message;
        match status {
            403 => assert!(msg.contains("Groq rejected the API key (403)"), "{msg}"),
            404 => assert_eq!(msg, GONE),
            _ => assert!(msg.contains(&format!("({status})")), "{msg}"),
        }
    }
}

#[tokio::test]
async fn groq_400_model_gone_is_model_unavailable() {
    let bodies = [
        json!({"error":{"message":"The model `openai/gpt-oss-120b` has been decommissioned","type":"invalid_request_error","code":"model_decommissioned"}}).to_string(),
        json!({"error":{"message":"x","code":"model_not_found"}}).to_string(),
        json!({"error":{"message":"The model `openai/gpt-oss-120b` does not exist or you do not have access to it."}}).to_string(),
    ];
    for body in bodies {
        let (res, _, _s, _) = run(Reply::status(400, &body)).await;
        let f = res.unwrap_err();
        assert_eq!(f.kind, ProviderFailureKind::ModelUnavailable, "{body}");
        assert_eq!(f.message, GONE);
    }
}

#[tokio::test]
async fn groq_other_400_is_http_with_snippet() {
    let body = json!({"error":{"message":"messages: too long"}}).to_string();
    let (res, _, _s, _) = run(Reply::status(400, &body)).await;
    let f = res.unwrap_err();
    assert_eq!(f.kind, ProviderFailureKind::Http { status: 400 });
    assert_eq!(
        f.message,
        "Groq returned an error (400): messages: too long"
    );
}

#[tokio::test]
async fn groq_key_echoed_in_error_body_is_redacted() {
    let body =
        json!({"error":{"message":format!("Authorization: Bearer {KEY} malformed")}}).to_string();
    let (res, _, _s, _) = run(Reply::status(400, &body)).await;
    assert_no_key(&res);
    assert!(res.unwrap_err().message.contains("***"));
}

#[tokio::test]
async fn groq_stream_drop_mid_body_is_stream_drop() {
    for framing in [Framing::Chunked, Framing::ContentLength] {
        let body = chunk(Some("Some text"), None);
        let reply = Reply::sse(hostile(body.as_bytes()))
            .framing(framing)
            .truncated();
        let (res, deltas, _s, _) = run(reply).await;
        assert_eq!(kind(&res), ProviderFailureKind::StreamDrop, "{framing:?}");
        assert_eq!(deltas, vec!["Some text"]);
    }
}

#[tokio::test]
async fn groq_close_before_response_is_connect() {
    let (res, _, _s, _) = run_script(Script::CloseWithoutResponse).await;
    assert_eq!(kind(&res), ProviderFailureKind::Connect);
}

#[tokio::test]
async fn groq_connection_refused_is_connect() {
    let p = provider(&format!("http://127.0.0.1:{}", dead_port()));
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, _rx) = mpsc::unbounded_channel();
    let res = p.stream(&req, tx).await;
    assert_eq!(kind(&res), ProviderFailureKind::Connect);
    assert_no_key(&res);
    assert!(res
        .unwrap_err()
        .message
        .starts_with("Could not connect to Groq"));
}

#[tokio::test]
async fn groq_request_bytes_identical_across_two_sends() {
    let server = TestServer::start(vec![
        Script::CloseWithoutResponse,
        Script::Reply(Reply::sse(vec![success_body("stop").into_bytes()])),
    ])
    .await;
    let p = provider(&server.origin());
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let (tx, _rx) = mpsc::unbounded_channel();
    let first = p.stream(&req, tx.clone()).await;
    assert_eq!(kind(&first), ProviderFailureKind::Connect);
    assert!(p.stream(&req, tx).await.is_ok());
    let recs = server.recorded();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].body, recs[1].body);
    assert_eq!(recs[0].body, req.body.to_vec());
    assert_eq!(
        recs[0].header("authorization"),
        recs[1].header("authorization")
    );
}

#[tokio::test]
async fn groq_request_body_is_exactly_spec_section_7() {
    let (_, _, server, _) = run(Reply::sse(vec![success_body("stop").into_bytes()])).await;
    let rec = &server.recorded()[0];
    let v: serde_json::Value = serde_json::from_slice(&rec.body).unwrap();
    assert_eq!(
        v,
        json!({
            "model": "openai/gpt-oss-120b",
            "messages": [
                {"role":"system","content":"You are me. Résumé: …\n\nAnswer briefly."},
                {"role":"user","content":"Tell me about yourself?"}
            ],
            "max_completion_tokens": 1024,
            "temperature": 0.7,
            "reasoning_effort": "low",
            "include_reasoning": false,
            "stream": true
        })
    );
    assert!(!String::from_utf8_lossy(&rec.body).contains("reasoning_format"));
}

#[tokio::test]
async fn groq_headers_present_and_correct() {
    let (_, _, server, _) = run(Reply::sse(vec![success_body("stop").into_bytes()])).await;
    let rec = &server.recorded()[0];
    assert_eq!(rec.method, "POST");
    assert_eq!(rec.path, "/openai/v1/chat/completions");
    assert_eq!(
        rec.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(rec.header("content-type"), Some("application/json"));
    assert!(rec.header("x-api-key").is_none());
}

#[test]
fn groq_prepared_request_debug_never_shows_key() {
    let p = provider("http://127.0.0.1:9");
    let req = p.build_request(&prompt(), &Secret::new(KEY));
    let dbg = format!("{req:?}");
    assert!(!dbg.contains("TEST-KEY"), "{dbg}");
    assert!(dbg.contains("***"));
}

#[tokio::test]
async fn groq_prewarm_gets_models_without_key() {
    let server = TestServer::start(vec![Script::Reply(Reply::status(401, "{}"))]).await;
    let p = provider(&server.origin());
    p.prewarm();
    let rec = server.next_request().await;
    assert_eq!(
        (rec.method.as_str(), rec.path.as_str()),
        ("GET", "/v1/models")
    );
    assert!(rec.header("authorization").is_none());
}

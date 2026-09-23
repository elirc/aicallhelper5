//! Shared HTTP plumbing: the one client, send + status/error mapping, the SSE
//! stream driver both providers run on, and the pre-warm throttle.

use std::sync::Mutex;
use std::time::Instant;

use callcore_contract::config::{
    PREWARM_THROTTLE, PREWARM_TIMEOUT, TRANSPORT_CONNECT_TIMEOUT, TRANSPORT_POOL_IDLE_TIMEOUT,
    TRANSPORT_READ_TIMEOUT,
};
use callcore_contract::ports::{
    HeaderValue, PreparedRequest, ProviderFailure, ProviderFailureKind, StreamOutcome,
};
use callcore_contract::Finish;
use tokio::sync::mpsc;

use crate::failure::{fail, snippet};
use crate::sse::{SseEvent, SseParser};

/// Error bodies are read up to this many bytes (the snippet is 200 chars).
const MAX_ERROR_BODY: usize = 64 * 1024;
/// How long a finished stream's leftover bytes are drained for pool reuse.
const RELEASE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(TRANSPORT_CONNECT_TIMEOUT)
        // Per-read inactivity timeout (reset on every chunk), not a total
        // deadline: the session's own watchdogs decide the total budget.
        .read_timeout(TRANSPORT_READ_TIMEOUT)
        .pool_idle_timeout(TRANSPORT_POOL_IDLE_TIMEOUT)
        .build()
        // Only fails if the TLS backend cannot initialise — a broken build,
        // not a runtime condition.
        .expect("HTTP client (rustls) must build")
}

/// Static facts a provider hands to the shared plumbing.
pub(crate) struct ProviderMeta {
    /// Brand name used in user copy, e.g. "Anthropic".
    pub name: &'static str,
    pub model: &'static str,
    /// What to suggest switching to when the model is gone, e.g. "Claude".
    pub alternative: &'static str,
    /// Does this non-2xx response mean "the model no longer exists"?
    pub model_unavailable: fn(status: u16, body: &str) -> bool,
}

impl ProviderMeta {
    pub fn model_unavailable_message(&self) -> String {
        format!(
            "The model {} is no longer available — switch the answer provider to {} in Settings or install the latest version.",
            self.model, self.alternative
        )
    }
}

/// Every secret value that could be echoed back by a server (the header value
/// itself and, for `Bearer x`, the bare token).
pub(crate) fn secrets_of(req: &PreparedRequest) -> Vec<String> {
    let mut out = Vec::new();
    for (_, v) in &req.headers {
        if let HeaderValue::Secret(s) = v {
            if let Some(tok) = s.strip_prefix("Bearer ") {
                out.push(tok.trim().to_owned());
            }
            out.push(s.clone());
        }
    }
    out.retain(|s| !s.is_empty());
    // Longest first so "Bearer k" is replaced before its substring "k".
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

/// Send `req`. `Err(Connect)` means no response byte arrived — safe to retry.
pub(crate) async fn send(
    client: &reqwest::Client,
    meta: &ProviderMeta,
    req: &PreparedRequest,
) -> Result<reqwest::Response, ProviderFailure> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &req.headers {
        let hname = reqwest::header::HeaderName::from_bytes(name.as_bytes());
        let hvalue = reqwest::header::HeaderValue::from_str(value.as_str());
        match (hname, hvalue) {
            (Ok(n), Ok(mut v)) => {
                if matches!(value, HeaderValue::Secret(_)) {
                    v.set_sensitive(true);
                }
                headers.insert(n, v);
            }
            _ => {
                // Never echo the value: it may be the key.
                return Err(fail(
                    ProviderFailureKind::Auth { status: 0 },
                    format!(
                        "The {} API key contains characters that can't be sent. Re-enter it in Settings.",
                        meta.name
                    ),
                ));
            }
        }
    }
    client
        .post(&req.url)
        .headers(headers)
        .body(req.body.clone())
        .send()
        .await
        .map_err(|e| {
            let why = if e.is_timeout() { " (timed out)" } else { "" };
            fail(
                ProviderFailureKind::Connect,
                format!(
                    "Could not connect to {}{why}. Check your internet connection and try again.",
                    meta.name
                ),
            )
        })
}

/// Map a non-2xx response to a failure (reads a bounded error body).
pub(crate) async fn status_failure(
    meta: &ProviderMeta,
    mut resp: reqwest::Response,
    secrets: &[String],
) -> ProviderFailure {
    let status = resp.status().as_u16();
    let mut raw = Vec::new();
    while raw.len() < MAX_ERROR_BODY {
        match resp.chunk().await {
            Ok(Some(b)) => raw.extend_from_slice(&b),
            _ => break,
        }
    }
    raw.truncate(MAX_ERROR_BODY);
    let body = String::from_utf8_lossy(&raw);
    map_status(meta, status, &body, secrets)
}

pub(crate) fn map_status(
    meta: &ProviderMeta,
    status: u16,
    body: &str,
    secrets: &[String],
) -> ProviderFailure {
    let snip = snippet(body, secrets);
    let detail = if snip.is_empty() {
        String::new()
    } else {
        format!(": {snip}")
    };
    let name = meta.name;
    match status {
        401 | 403 => fail(
            ProviderFailureKind::Auth { status },
            format!("{name} rejected the API key ({status}). Check the key in Settings."),
        ),
        429 | 529 => fail(
            ProviderFailureKind::RateLimit { status },
            format!(
                "{name} is rate-limiting or overloaded ({status}){detail}. Wait a few seconds and try again."
            ),
        ),
        s if (meta.model_unavailable)(s, body) => {
            fail(ProviderFailureKind::ModelUnavailable, meta.model_unavailable_message())
        }
        _ => fail(
            ProviderFailureKind::Http { status },
            format!("{name} returned an error ({status}){detail}"),
        ),
    }
}

/// Result of feeding one SSE event to a provider's stream handler.
pub(crate) enum Flow {
    Continue,
    /// The provider's real terminal event arrived: stop reading now.
    Finished,
}

/// Collects the answer and forwards deltas to the session.
pub(crate) struct Emitter {
    deltas: mpsc::UnboundedSender<String>,
    pub answer: String,
}

impl Emitter {
    pub fn emit(&mut self, text: &str) -> Result<(), ProviderFailure> {
        if text.is_empty() {
            return Ok(());
        }
        self.deltas
            .send(text.to_owned())
            .map_err(|_| fail(ProviderFailureKind::Aborted, "The answer was cancelled."))?;
        self.answer.push_str(text);
        Ok(())
    }
}

pub(crate) trait StreamHandler {
    fn on_event(&mut self, ev: &SseEvent, out: &mut Emitter) -> Result<Flow, ProviderFailure>;
    /// Stop reading after the current network chunk (Groq `[DONE]`).
    fn stop_after_chunk(&self) -> bool {
        false
    }
    /// `Some` once the provider's real terminal signal has been seen.
    fn terminal(&self) -> Option<Finish>;
}

/// Send, check status, then drive the SSE stream through `handler`.
pub(crate) async fn run_stream<H: StreamHandler>(
    client: &reqwest::Client,
    meta: &ProviderMeta,
    req: &PreparedRequest,
    deltas: mpsc::UnboundedSender<String>,
    mut handler: H,
) -> Result<StreamOutcome, ProviderFailure> {
    let secrets = secrets_of(req);
    let mut resp = send(client, meta, req).await?;
    if !resp.status().is_success() {
        return Err(status_failure(meta, resp, &secrets).await);
    }
    let mut out = Emitter {
        deltas,
        answer: String::new(),
    };
    let mut parser = SseParser::new();
    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                for ev in parser.feed(&bytes) {
                    if let Flow::Finished = handler.on_event(&ev, &mut out)? {
                        release(resp);
                        return complete(meta, &handler, out);
                    }
                }
                if handler.stop_after_chunk() {
                    release(resp);
                    return complete(meta, &handler, out);
                }
            }
            Ok(None) => {
                for ev in parser.finish() {
                    if let Flow::Finished = handler.on_event(&ev, &mut out)? {
                        break;
                    }
                }
                if handler.terminal().is_some() {
                    return complete(meta, &handler, out);
                }
                return Err(fail(
                    ProviderFailureKind::Incomplete,
                    format!(
                        "The {} answer stream ended before it finished. Try again.",
                        meta.name
                    ),
                ));
            }
            Err(e) => {
                if handler.terminal().is_some() {
                    // The terminal signal already arrived; a late drop is noise.
                    return complete(meta, &handler, out);
                }
                let why = if e.is_timeout() {
                    " (no data for 75 seconds)"
                } else {
                    ""
                };
                return Err(fail(
                    ProviderFailureKind::StreamDrop,
                    format!(
                        "The connection to {} dropped while the answer was streaming{why}. Try again.",
                        meta.name
                    ),
                ));
            }
        }
    }
}

/// The terminal event arrived before the body's end: finish reading the
/// (tiny) remainder in the background so the connection goes back to the pool
/// for the next answer instead of paying a fresh TLS handshake.
fn release(mut resp: reqwest::Response) {
    tokio::spawn(async move {
        let _ = tokio::time::timeout(RELEASE_DRAIN_TIMEOUT, async {
            while let Ok(Some(_)) = resp.chunk().await {}
        })
        .await;
    });
}

fn complete<H: StreamHandler>(
    meta: &ProviderMeta,
    handler: &H,
    out: Emitter,
) -> Result<StreamOutcome, ProviderFailure> {
    let finish = handler.terminal().unwrap_or(Finish::Complete);
    if out.answer.trim().is_empty() {
        return Err(fail(
            ProviderFailureKind::EmptyAnswer,
            format!(
                "{} returned an empty answer. Try again or rephrase the question.",
                meta.name
            ),
        ));
    }
    Ok(StreamOutcome {
        finish,
        answer: out.answer,
    })
}

// ───────────────────────────── pre-warm ─────────────────────────────

/// Allows at most one pre-warm per [`PREWARM_THROTTLE`].
#[derive(Debug, Default)]
pub(crate) struct Throttle {
    last: Mutex<Option<Instant>>,
}

impl Throttle {
    /// True (and records `now`) when a pre-warm may fire at `now`.
    pub fn try_fire(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        match *last {
            Some(t) if now.saturating_duration_since(t) < PREWARM_THROTTLE => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

/// Throttled, fire-and-forget `GET <origin>/v1/models` with no key and a 3 s
/// timeout. Never blocks, never panics (no-op outside a tokio runtime).
pub(crate) fn prewarm(client: &reqwest::Client, origin: &str, throttle: &Throttle) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if !throttle.try_fire(Instant::now()) {
        return;
    }
    let req = client
        .get(format!("{}/v1/models", origin.trim_end_matches('/')))
        .timeout(PREWARM_TIMEOUT);
    handle.spawn(async move {
        // Drain the (tiny, 401) body so the connection returns to the pool.
        if let Ok(resp) = req.send().await {
            let _ = resp.bytes().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn meta() -> ProviderMeta {
        ProviderMeta {
            name: "Groq",
            model: "openai/gpt-oss-120b",
            alternative: "Claude",
            model_unavailable: |s, _| s == 404,
        }
    }

    #[test]
    fn throttle_allows_one_fire_per_two_seconds() {
        let t = Throttle::default();
        let t0 = Instant::now();
        assert!(t.try_fire(t0));
        assert!(!t.try_fire(t0 + Duration::from_millis(1999)));
        assert!(t.try_fire(t0 + Duration::from_millis(2000)));
        assert!(!t.try_fire(t0 + Duration::from_millis(2500)));
    }

    #[test]
    fn prewarm_without_runtime_is_a_noop() {
        let client = build_client();
        let t = Throttle::default();
        prewarm(&client, "http://127.0.0.1:9", &t);
        // Not recorded: a later call inside a runtime may still fire.
        assert!(t.try_fire(Instant::now()));
    }

    #[test]
    fn secrets_of_extracts_bearer_token_and_raw_value() {
        let req = PreparedRequest {
            url: "u".into(),
            headers: vec![
                (
                    "authorization".into(),
                    HeaderValue::Secret("Bearer gsk_abc".into()),
                ),
                ("x".into(), HeaderValue::Plain("p".into())),
            ],
            body: bytes::Bytes::new(),
        };
        assert_eq!(
            secrets_of(&req),
            vec!["Bearer gsk_abc".to_owned(), "gsk_abc".to_owned()]
        );
    }

    #[test]
    fn map_status_kinds_and_copy() {
        let m = meta();
        let f = map_status(&m, 403, "", &[]);
        assert_eq!(f.kind, ProviderFailureKind::Auth { status: 403 });
        assert!(
            f.message.contains("Groq rejected the API key (403)"),
            "{}",
            f.message
        );
        let f = map_status(&m, 529, r#"{"error":{"message":"Overloaded"}}"#, &[]);
        assert_eq!(f.kind, ProviderFailureKind::RateLimit { status: 529 });
        assert!(f.message.contains("(529): Overloaded"), "{}", f.message);
        let f = map_status(&m, 404, "", &[]);
        assert_eq!(f.kind, ProviderFailureKind::ModelUnavailable);
        let f = map_status(&m, 502, "<html>Bad gateway</html>", &[]);
        assert_eq!(f.kind, ProviderFailureKind::Http { status: 502 });
        assert_eq!(
            f.message,
            "Groq returned an error (502): <html>Bad gateway</html>"
        );
    }
}

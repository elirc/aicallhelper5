//! Anthropic Messages API (default provider, spec §7).

use async_trait::async_trait;
use callcore_contract::config::MAX_TOKENS;
use callcore_contract::ports::{
    AnswerProvider, HeaderValue, PreparedRequest, PromptParts, ProviderFailure,
    ProviderFailureKind, StreamOutcome,
};
use callcore_contract::{Finish, Secret};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::failure::{clean, fail};
use crate::http::{self, Emitter, Flow, ProviderMeta, StreamHandler, Throttle};
use crate::sse::SseEvent;

pub const ID: &str = "anthropic";
pub const DISPLAY_NAME: &str = "Claude Haiku 4.5 (recommended)";
pub const KEY_ID: &str = "anthropic";
pub const MODEL: &str = "claude-haiku-4-5";
pub const API_VERSION: &str = "2023-06-01";

pub(crate) const META: ProviderMeta = ProviderMeta {
    name: "Anthropic",
    model: MODEL,
    alternative: "Groq",
    model_unavailable: |status, _| status == 404,
};

pub struct AnthropicProvider {
    client: reqwest::Client,
    origin: String,
    throttle: Throttle,
}

impl AnthropicProvider {
    pub fn new(client: reqwest::Client, origin: &str) -> Self {
        Self {
            client,
            origin: origin.trim_end_matches('/').to_owned(),
            throttle: Throttle::default(),
        }
    }
}

/// The exact §7 request body.
pub fn request_body(prompt: &PromptParts) -> Value {
    json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "system": [
            {
                "type": "text",
                "text": prompt.cached_prefix,
                "cache_control": {"type": "ephemeral"}
            },
            {"type": "text", "text": prompt.style_suffix}
        ],
        "messages": [
            {"role": "user", "content": prompt.user_message}
        ]
    })
}

#[async_trait]
impl AnswerProvider for AnthropicProvider {
    fn id(&self) -> &'static str {
        ID
    }
    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }
    fn key_id(&self) -> &'static str {
        KEY_ID
    }
    fn model(&self) -> &'static str {
        MODEL
    }

    fn build_request(&self, prompt: &PromptParts, key: &Secret) -> PreparedRequest {
        let body = serde_json::to_vec(&request_body(prompt)).unwrap_or_default();
        PreparedRequest {
            url: format!("{}/v1/messages", self.origin),
            headers: vec![
                (
                    "x-api-key".into(),
                    HeaderValue::Secret(key.expose().to_owned()),
                ),
                (
                    "anthropic-version".into(),
                    HeaderValue::Plain(API_VERSION.into()),
                ),
                (
                    "content-type".into(),
                    HeaderValue::Plain("application/json".into()),
                ),
            ],
            body: body.into(),
        }
    }

    async fn stream(
        &self,
        req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure> {
        let secrets = http::secrets_of(req);
        let handler = Handler {
            stop_reason: None,
            stopped: false,
            secrets,
        };
        http::run_stream(&self.client, &META, req, deltas, handler).await
    }

    fn prewarm(&self) {
        http::prewarm(&self.client, &self.origin, &self.throttle);
    }
}

struct Handler {
    stop_reason: Option<String>,
    stopped: bool,
    secrets: Vec<String>,
}

impl StreamHandler for Handler {
    fn on_event(&mut self, ev: &SseEvent, out: &mut Emitter) -> Result<Flow, ProviderFailure> {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
            tracing::debug!(len = ev.data.len(), "anthropic: ignoring non-JSON SSE data");
            return Ok(Flow::Continue);
        };
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .or(ev.event.as_deref())
            .unwrap_or("");
        match ty {
            "content_block_delta" => {
                if v.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") {
                    if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                        out.emit(t)?;
                    }
                }
            }
            "message_delta" => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(r.to_owned());
                }
            }
            "message_stop" => {
                self.stopped = true;
                return Ok(Flow::Finished);
            }
            "error" => return Err(stream_error(&v, &self.secrets)),
            _ => {} // message_start, content_block_start/stop, ping, unknown
        }
        Ok(Flow::Continue)
    }

    fn terminal(&self) -> Option<Finish> {
        if !self.stopped {
            return None;
        }
        Some(match self.stop_reason.as_deref() {
            Some("max_tokens") | Some("model_context_window_exceeded") => Finish::Truncated,
            Some("refusal") => Finish::Refused,
            _ => Finish::Complete,
        })
    }
}

fn stream_error(v: &Value, secrets: &[String]) -> ProviderFailure {
    let ty = v
        .pointer("/error/type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let msg = clean(
        v.pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or(ty),
        secrets,
    );
    let detail = if msg.is_empty() {
        String::new()
    } else {
        format!(": {msg}")
    };
    match ty {
        "overloaded_error" => fail(
            ProviderFailureKind::RateLimit { status: 529 },
            format!("Anthropic is rate-limiting or overloaded (529){detail}. Wait a few seconds and try again."),
        ),
        "rate_limit_error" => fail(
            ProviderFailureKind::RateLimit { status: 429 },
            format!("Anthropic is rate-limiting or overloaded (429){detail}. Wait a few seconds and try again."),
        ),
        _ => fail(
            ProviderFailureKind::ProviderError,
            format!("Anthropic reported an error mid-answer{detail}"),
        ),
    }
}
